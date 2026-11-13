//! Windows service integration: install / uninstall with the Service Control Manager (SCM) and
//! the SCM entry point (`service dispatch`, launched by the SCM, never by hand).
//!
//! Stop and system shutdown are graceful: the service reports `StopPending`, stops accepting
//! connections and lets in-flight requests finish (bounded by the request timeout).

use std::error::Error as _;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::config::Config;
use crate::logging;
use crate::reload::ReloadHandle;
use crate::server::{self, RunHooks};

pub const DEFAULT_NAME: &str = "veriface";
/// The default account: the service's own virtual account `NT SERVICE\<name>` (least privilege,
/// a SID of its own, no password). It needs read access to the install and configuration
/// directories and write access to the log directory; `Install-Veriface.ps1` grants them.
pub fn default_account(name: &str) -> String {
    format!(r"NT SERVICE\{name}")
}
const DISPLAY_NAME: &str = "Veriface";
const DESCRIPTION: &str =
    "Veriface: face quality checks, ID-document portraits and 1:1 verification over HTTP.";
/// How long the SCM should wait for start and stop transitions.
const PENDING_WAIT_HINT: Duration = Duration::from_secs(60);

/// What `dispatch` hands to the service main function (the SCM calls it on its own thread).
static DISPATCH: OnceLock<(PathBuf, String)> = OnceLock::new();

/// Validate the configuration, then register the service (automatic start, restart on failure).
pub fn install(config: &Path, name: &str, account: &str) -> Result<(), String> {
    validate_name(name)?;
    let config_path = std::path::absolute(config)
        .map_err(|e| format!("cannot resolve `{}`: {e}", config.display()))?;
    let parsed = Config::load(&config_path).map_err(|e| format!("invalid configuration: {e}"))?;
    server::check(&parsed).map_err(|e| format!("invalid configuration: {e}"))?;

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|e| scm_error("cannot connect to the Service Control Manager", &e))?;
    let executable_path =
        std::env::current_exe().map_err(|e| format!("cannot locate this executable: {e}"))?;
    let local_system = account.eq_ignore_ascii_case("LocalSystem");
    let info = ServiceInfo {
        name: OsString::from(name),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path,
        launch_arguments: vec![
            "service".into(),
            "dispatch".into(),
            "--config".into(),
            config_path.clone().into_os_string(),
            "--name".into(),
            name.into(),
        ],
        dependencies: Vec::new(),
        account_name: (!local_system).then(|| OsString::from(account)),
        account_password: None,
    };
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
        .map_err(|e| scm_error(&format!("cannot create service `{name}`"), &e))?;
    service
        .set_description(DESCRIPTION)
        .map_err(|e| scm_error("cannot set the description", &e))?;
    service
        .update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 60 * 60)),
            reboot_msg: None,
            command: None,
            actions: Some(
                [5, 30, 120]
                    .into_iter()
                    .map(|seconds| ServiceAction {
                        action_type: ServiceActionType::Restart,
                        delay: Duration::from_secs(seconds),
                    })
                    .collect(),
            ),
        })
        .map_err(|e| scm_error("cannot set the recovery actions", &e))?;
    service
        .set_failure_actions_on_non_crash_failures(true)
        .map_err(|e| scm_error("cannot set the recovery actions", &e))?;

    println!(
        "Installed service `{name}` ({DISPLAY_NAME}) running as {} with config {}.",
        if local_system { "LocalSystem" } else { account },
        config_path.display()
    );
    println!("Start it with: sc.exe start {name}   (or: Start-Service {name})");
    Ok(())
}

/// Stop the service if it is running, then delete it.
pub fn uninstall(name: &str) -> Result<(), String> {
    validate_name(name)?;
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| scm_error("cannot connect to the Service Control Manager", &e))?;
    let service = manager
        .open_service(
            name,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        )
        .map_err(|e| scm_error(&format!("cannot open service `{name}`"), &e))?;
    let state = |s: &windows_service::service::Service| {
        s.query_status()
            .map(|status| status.current_state)
            .map_err(|e| scm_error("cannot query the service", &e))
    };
    if state(&service)? != ServiceState::Stopped {
        // Fails harmlessly when a stop is already pending.
        let _ = service.stop();
        let deadline = Instant::now() + Duration::from_secs(90);
        while state(&service)? != ServiceState::Stopped {
            if Instant::now() > deadline {
                return Err(format!(
                    "service `{name}` did not stop within 90 s; it was not removed"
                ));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    service
        .delete()
        .map_err(|e| scm_error(&format!("cannot delete service `{name}`"), &e))?;
    println!("Removed service `{name}`.");
    Ok(())
}

define_windows_service!(ffi_service_main, service_main);

/// SCM entry point: blocks until the service stops.
pub fn dispatch(config: PathBuf, name: String) -> Result<(), String> {
    validate_name(&name)?;
    DISPATCH
        .set((config, name.clone()))
        .map_err(|_| "the service dispatcher is already running".to_owned())?;
    service_dispatcher::start(&name, ffi_service_main).map_err(|e| {
        scm_error(
            "cannot connect to the Service Control Manager (this command is run by the SCM; \
             use `veriface run` interactively)",
            &e,
        )
    })
}

fn service_main(_arguments: Vec<OsString>) {
    let Some((config, name)) = DISPATCH.get() else {
        return;
    };
    if let Err(e) = run_service(config, name) {
        tracing::error!(error = %e, "service stopped with an error");
    }
}

fn run_service(config_path: &Path, name: &str) -> Result<(), String> {
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    // `sc.exe control <name> paramchange` reloads the API keys and profiles.
    let reloader: Arc<OnceLock<ReloadHandle>> = Arc::new(OnceLock::new());
    let reload_target = reloader.clone();
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::ParamChange => {
            if let Some(handle) = reload_target.get() {
                handle.request("paramchange");
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let handle = service_control_handler::register(name, handler)
        .map_err(|e| scm_error("cannot register the service control handler", &e))?;
    let status = StatusReporter(handle);
    status.set(ServiceState::StartPending, PENDING_WAIT_HINT);

    let result = (|| {
        let config = Config::load(config_path).map_err(|e| e.to_string())?;
        logging::init(&config.logging)?;
        logging::install_panic_hook();
        tracing::info!(service = name, config = %config_path.display(), "starting as a Windows service");
        // In-flight requests may take up to the request timeout to drain (plus the server's
        // shutdown margin); tell the SCM to wait at least that long.
        let stop_wait =
            (config.server.request_timeout() + Duration::from_secs(15)).max(PENDING_WAIT_HINT);
        let hooks = RunHooks {
            on_listening: Some(Box::new(move || {
                status.set(ServiceState::Running, Duration::ZERO)
            })),
            on_stopping: Some(Box::new(move || {
                status.set(ServiceState::StopPending, stop_wait);
            })),
            on_reloader: Some(Box::new(move |handle| {
                let _ = reloader.set(handle);
            })),
        };
        let shutdown = async move {
            let _ = stop_rx.wait_for(|stop| *stop).await;
        };
        server::run(config, shutdown, hooks).map_err(|e| e.to_string())
    })();

    status.stopped(result.is_ok());
    result
}

#[derive(Clone, Copy)]
struct StatusReporter(ServiceStatusHandle);

impl StatusReporter {
    fn set(&self, state: ServiceState, wait_hint: Duration) {
        let controls_accepted = if state == ServiceState::Running {
            ServiceControlAccept::STOP
                | ServiceControlAccept::SHUTDOWN
                | ServiceControlAccept::PARAM_CHANGE
        } else {
            ServiceControlAccept::empty()
        };
        self.report(
            state,
            controls_accepted,
            ServiceExitCode::NO_ERROR,
            wait_hint,
        );
    }

    fn stopped(&self, ok: bool) {
        let exit_code = if ok {
            ServiceExitCode::NO_ERROR
        } else {
            ServiceExitCode::ServiceSpecific(1)
        };
        self.report(
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
            exit_code,
            Duration::ZERO,
        );
    }

    fn report(
        &self,
        state: ServiceState,
        controls_accepted: ServiceControlAccept,
        exit_code: ServiceExitCode,
        wait_hint: Duration,
    ) {
        let result = self.0.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted,
            exit_code,
            checkpoint: 0,
            wait_hint,
            process_id: None,
        });
        if let Err(e) = result {
            tracing::warn!(error = %scm_error("cannot report the service status", &e), "scm");
        }
    }
}

/// Service names: 1-64 characters from `A-Z a-z 0-9 . _ -`.
fn validate_name(name: &str) -> Result<(), String> {
    let ok = (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "service name `{name}` must be 1-64 characters from A-Z, a-z, 0-9, '.', '_' and '-'"
        ))
    }
}

fn scm_error(context: &str, e: &windows_service::Error) -> String {
    let mut message = format!("{context}: {e}");
    if let Some(source) = e.source() {
        message.push_str(&format!(" ({source})"));
        if let Some(io) = source.downcast_ref::<std::io::Error>()
            && io.raw_os_error() == Some(5)
        {
            message.push_str("; run this from an elevated (Administrator) prompt");
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_names_are_restricted() {
        assert!(validate_name("veriface").is_ok());
        assert!(validate_name("Veriface.Face_2").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("bad name").is_err());
        assert!(validate_name(r"bad\name").is_err());
        assert!(validate_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn the_default_account_is_the_virtual_service_account() {
        assert_eq!(default_account("veriface"), r"NT SERVICE\veriface");
    }
}
