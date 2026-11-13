//! `veriface`: the Veriface service.
//!
//! - `veriface run --config <path>`: run in the foreground (Linux containers, development).
//! - `veriface check --config <path>`: everything startup checks (configuration, ONNX Runtime,
//!   models, profiles, keys), then exit: 0 when the service would start.
//! - `veriface healthcheck (--url <url> | --config <path>)`: probe `/health/ready`; exit 0 on
//!   200 (container `HEALTHCHECK`s, install scripts).
//! - `veriface service install --config <path> [--name veriface]` (Windows): register with
//!   the Service Control Manager.
//! - `veriface service uninstall [--name veriface]` (Windows).

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use face_server::config::Config;
use face_server::{healthcheck, logging, server};

#[derive(Parser)]
#[command(
    name = "veriface",
    version,
    about = "Veriface: face quality, document portraits and 1:1 verification over HTTP"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the service in the foreground until Ctrl-C (or SIGTERM).
    Run {
        /// Path to the TOML configuration file.
        #[arg(long)]
        config: PathBuf,
    },
    /// Check the configuration, ONNX Runtime, the models, the profiles and the keys, then exit:
    /// 0 when the service would start, 2 (with every problem listed) when it would not.
    Check {
        /// Path to the TOML configuration file.
        #[arg(long)]
        config: PathBuf,
    },
    /// Probe a running service (`/health/ready` by default): exit 0 on 200, else 1. Sends the
    /// key in VERIFACE_HEALTHCHECK_KEY as X-API-Key when set.
    Healthcheck {
        /// URL to probe, e.g. http://127.0.0.1:8080/health/ready.
        #[arg(long, conflicts_with = "config", required_unless_present = "config")]
        url: Option<String>,
        /// Probe /health/ready on the listener this configuration binds.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Connect and read timeout, in milliseconds.
        #[arg(long, default_value_t = 3000)]
        timeout_ms: u64,
    },
    /// Manage the Windows service.
    #[cfg(windows)]
    #[command(subcommand)]
    Service(ServiceCommand),
}

#[cfg(windows)]
#[derive(Subcommand)]
enum ServiceCommand {
    /// Validate the configuration and register the service (auto start, restart on failure).
    Install {
        /// Path to the TOML configuration file (stored as an absolute path).
        #[arg(long)]
        config: PathBuf,
        /// Service name.
        #[arg(long, default_value = face_server::service::DEFAULT_NAME)]
        name: String,
        /// Account the service runs as. Default: the virtual account `NT SERVICE\<name>`. Also
        /// `NT AUTHORITY\LocalService` or a gMSA (`DOMAIN\name$`); `LocalSystem` is accepted but
        /// not recommended.
        #[arg(long)]
        account: Option<String>,
    },
    /// Stop the service if it is running, then remove it.
    Uninstall {
        /// Service name.
        #[arg(long, default_value = face_server::service::DEFAULT_NAME)]
        name: String,
    },
    /// Entry point used by the Service Control Manager. Not for interactive use.
    #[command(hide = true)]
    Dispatch {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        name: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Run { config } => run_foreground(config),
        Command::Check { config } => check(config),
        Command::Healthcheck {
            url,
            config,
            timeout_ms,
        } => probe(url, config, Duration::from_millis(timeout_ms)),
        #[cfg(windows)]
        Command::Service(command) => {
            use face_server::service;
            let result = match command {
                ServiceCommand::Install {
                    config,
                    name,
                    account,
                } => {
                    let account = account.unwrap_or_else(|| service::default_account(&name));
                    service::install(&config, &name, &account)
                }
                ServiceCommand::Uninstall { name } => service::uninstall(&name),
                ServiceCommand::Dispatch { config, name } => service::dispatch(config, name),
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("veriface: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn run_foreground(config_path: PathBuf) -> ExitCode {
    let config = match Config::load(&config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("veriface: invalid configuration: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = logging::init(&config.logging) {
        eprintln!("veriface: {e}");
        return ExitCode::from(2);
    }
    logging::install_panic_hook();
    match server::run(
        config,
        server::shutdown_signal(),
        server::RunHooks::default(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "veriface stopped with an error");
            eprintln!("veriface: {e}");
            ExitCode::FAILURE
        }
    }
}

fn check(config_path: PathBuf) -> ExitCode {
    let result = Config::load(&config_path).and_then(|config| {
        server::check(&config)?;
        Ok(config)
    });
    match result {
        Ok(config) => {
            println!(
                "veriface: {} is valid ({} API keys; engine {:?}); the service would start",
                config_path.display(),
                config.api_keys.len(),
                config.engine.kind
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("veriface: invalid configuration: {e}");
            ExitCode::from(2)
        }
    }
}

fn probe(url: Option<String>, config_path: Option<PathBuf>, timeout: Duration) -> ExitCode {
    let target = match (url, config_path) {
        (Some(url), _) => healthcheck::Target::from_url(&url),
        (None, Some(path)) => Config::load(&path)
            .map_err(|e| format!("invalid configuration: {e}"))
            .and_then(|config| healthcheck::Target::from_config(&config)),
        (None, None) => Err("give --url or --config".to_owned()),
    };
    let key = std::env::var(healthcheck::KEY_ENV)
        .ok()
        .filter(|k| !k.is_empty());
    match target.and_then(|t| healthcheck::probe(&t, key.as_deref(), timeout)) {
        Ok(line) => {
            println!("healthy: {line}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("unhealthy: {e}");
            ExitCode::FAILURE
        }
    }
}
