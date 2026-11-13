//! Startup and the serve loop (docs/architecture.md section 5): validate the configuration, load
//! the profiles and keys, build the engine, bind, warm up, then report ready. Stops gracefully on
//! the shutdown signal, or with an error when warm-up fails.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::TcpListener;
use tracing::{error, info, warn};

use crate::api::{self, App, Settings};
use crate::auth::ApiKeys;
use crate::config::{Config, EngineKind, StartupError};
use crate::engine::{Engine, StubEngine};
use crate::net::{self, ListenOptions};
use crate::pipeline::PipelineEngine;
use crate::profiles::ProfileStore;
use crate::reload;
use crate::tls::TlsState;

/// Extra time, beyond the request timeout, that in-flight requests get to finish on shutdown.
const SHUTDOWN_MARGIN: Duration = Duration::from_secs(5);

/// The engine selected by `engine.kind`: the vision pipeline (default) or the stub.
pub fn build_engine(config: &Config) -> Result<Arc<dyn Engine>, StartupError> {
    match config.engine.kind {
        EngineKind::Pipeline => Ok(Arc::new(PipelineEngine::load(config)?)),
        EngineKind::Stub => {
            warn!(
                "STUB ENGINE (engine.kind = \"stub\"): no models; every check is undetermined. Do not deploy."
            );
            Ok(Arc::new(StubEngine::new()))
        }
    }
}

/// Load the profiles and keys and assemble the app (not yet ready). Reports every problem.
pub fn build_app(config: &Config, engine: Arc<dyn Engine>) -> Result<Arc<App>, StartupError> {
    let profiles = ProfileStore::load_dir(&config.paths.profiles)
        .map_err(|problems| StartupError { problems })?;
    let keys = ApiKeys::from_config(&config.api_keys, &profiles)
        .map_err(|problems| StartupError { problems })?;
    let problems: Vec<String> = profiles
        .iter()
        .filter_map(|p| engine.check_profile(&p.profile).err())
        .collect();
    StartupError::from_problems(problems)?;
    Ok(App::new(engine, profiles, keys, settings(config)))
}

pub fn settings(config: &Config) -> Settings {
    Settings {
        request_timeout: config.server.request_timeout(),
        health_requires_key: config.health_requires_key,
        metrics_enabled: config.metrics.enabled,
        max_concurrency: config.server.effective_concurrency(),
        queue_limit: config.server.queue_limit,
        metrics_on_api: config.server.management_bind.is_none(),
        // Validated with the configuration.
        trusted_proxies: config.server.trusted_proxy_ranges().unwrap_or_default(),
    }
}

/// Everything `run` would check before binding (used by `service install`). With the pipeline
/// engine this loads ONNX Runtime and verifies the models.
pub fn check(config: &Config) -> Result<(), StartupError> {
    let engine = build_engine(config)?;
    build_app(config, engine).map(|_| ())
}

/// Callbacks for the Windows service.
#[derive(Default)]
pub struct RunHooks {
    /// The listener is bound.
    pub on_listening: Option<Box<dyn FnOnce() + Send>>,
    /// Shutdown has started; in-flight requests are draining.
    pub on_stopping: Option<Box<dyn FnOnce() + Send>>,
    /// Receives the handle that triggers a hot reload (the service's `paramchange` control).
    pub on_reloader: Option<Box<dyn FnOnce(reload::ReloadHandle) + Send>>,
}

/// Run the service until `shutdown` completes (or warm-up fails). Blocks the calling thread.
pub fn run(
    config: Config,
    shutdown: impl Future<Output = ()> + Send + 'static,
    hooks: RunHooks,
) -> Result<(), StartupError> {
    let bind = config.server.bind_addr().map_err(StartupError::new)?;
    let management = config.server.management_addr().map_err(StartupError::new)?;
    // Before the keys and profiles are loaded: a change from here on triggers a reload.
    let fingerprint = config
        .source
        .as_deref()
        .map(|path| reload::Fingerprint::of(path, &config))
        .unwrap_or_default();
    let tls = config
        .server
        .tls
        .as_ref()
        .map(|t| TlsState::load(t).map(Arc::new))
        .transpose()
        .map_err(StartupError::new)?;
    let engine = build_engine(&config)?;
    let app = build_app(&config, engine)?;
    let concurrency = app.settings.max_concurrency;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("veriface")
        .max_blocking_threads(concurrency + 8)
        .build()
        .map_err(|e| StartupError::new(format!("cannot start the async runtime: {e}")))?;

    for note in app.catalog().keys.validity_notes(crate::auth::now_unix()) {
        warn!("{note}");
    }
    let options = ServeOptions {
        max_connections: config.server.max_connections,
        header_timeout: config.server.header_timeout(),
        tls: tls.clone(),
        grace: app.settings.request_timeout + SHUTDOWN_MARGIN,
    };
    let (reloader, reload_requests) = reload::channel();
    if let Some(on_reloader) = hooks.on_reloader {
        on_reloader(reloader.clone());
    }
    let result = runtime.block_on(async move {
        let listener = TcpListener::bind(bind)
            .await
            .map_err(|e| StartupError::new(format!("cannot listen on {bind}: {e}")))?;
        let management = match management {
            Some(addr) => Some(TcpListener::bind(addr).await.map_err(|e| {
                StartupError::new(format!("cannot listen on {addr} (management): {e}"))
            })?),
            None => None,
        };
        tokio::spawn(reload::run(
            app.clone(),
            config,
            tls,
            fingerprint,
            reload_requests,
        ));
        #[cfg(unix)]
        tokio::spawn(reload_on_sighup(reloader.clone()));
        #[cfg(not(unix))]
        let _ = &reloader;
        info!(
            address = %bind,
            tls = options.tls.is_some(),
            management = management.as_ref().and_then(|l| l.local_addr().ok()).map(tracing::field::display),
            profiles = app.catalog().profiles.len(),
            max_concurrency = concurrency,
            queue_limit = app.settings.queue_limit,
            engine = %app.engine.info().version,
            "listening"
        );
        if let Some(on_listening) = hooks.on_listening {
            on_listening();
        }
        serve_with(app, listener, management, options, shutdown, hooks.on_stopping)
            .await
            .map_err(StartupError::new)
    });
    // Blocking work that outlived its request (timeouts) gets a moment to finish.
    runtime.shutdown_timeout(SHUTDOWN_MARGIN);
    result
}

/// How [`serve_with`] runs its listeners.
#[derive(Clone)]
pub struct ServeOptions {
    pub max_connections: usize,
    pub header_timeout: Duration,
    /// TLS on the API listener.
    pub tls: Option<Arc<TlsState>>,
    /// How long in-flight requests may take to finish after the shutdown signal.
    pub grace: Duration,
}

impl ServeOptions {
    /// Plain HTTP with the default limits.
    pub fn plain(grace: Duration) -> Self {
        ServeOptions {
            max_connections: 1024,
            header_timeout: Duration::from_secs(10),
            tls: None,
            grace,
        }
    }
}

/// Serve on `listener` (plain HTTP, default limits): see [`serve_with`].
pub async fn serve(
    app: Arc<App>,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
    grace: Duration,
    on_stopping: Option<Box<dyn FnOnce() + Send>>,
) -> Result<(), String> {
    serve_with(
        app,
        listener,
        None,
        ServeOptions::plain(grace),
        shutdown,
        on_stopping,
    )
    .await
}

/// Serve the API on `listener` (and health and metrics on `management`): warm up the engine in
/// the background, mark the app ready, and stop on `shutdown` (graceful, at most
/// `options.grace`) or with an error when warm-up fails.
pub async fn serve_with(
    app: Arc<App>,
    listener: TcpListener,
    management: Option<TcpListener>,
    options: ServeOptions,
    shutdown: impl Future<Output = ()> + Send + 'static,
    on_stopping: Option<Box<dyn FnOnce() + Send>>,
) -> Result<(), String> {
    let (fatal_tx, fatal_rx) = tokio::sync::oneshot::channel::<String>();
    {
        let app = app.clone();
        tokio::spawn(async move {
            let engine = app.engine.clone();
            let started = Instant::now();
            match tokio::task::spawn_blocking(move || engine.warm_up()).await {
                Ok(Ok(())) => {
                    app.set_ready(true);
                    info!(
                        warm_up_ms = started.elapsed().as_millis() as u64,
                        "engine warmed up; ready"
                    );
                }
                Ok(Err(e)) => {
                    let _ = fatal_tx.send(format!("engine warm-up failed: {e}"));
                }
                Err(_) => {
                    let _ = fatal_tx.send("engine warm-up panicked".to_owned());
                }
            }
        });
    }
    let fatal = async move {
        match fatal_rx.await {
            Ok(message) => message,
            // Warm-up succeeded and dropped the sender: never fires.
            Err(_) => std::future::pending().await,
        }
    };

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut servers = tokio::task::JoinSet::new();
    {
        let (app, stop) = (app.clone(), stop_rx.clone());
        let listen = ListenOptions {
            name: "api",
            max_connections: options.max_connections,
            header_timeout: options.header_timeout,
            tls: options.tls.clone(),
        };
        servers.spawn(async move {
            net::serve_connections(
                listener,
                api::router(app.clone()),
                listen,
                &app.metrics,
                stop,
            )
            .await;
        });
    }
    if let Some(management) = management {
        let (app, stop) = (app.clone(), stop_rx.clone());
        let listen = ListenOptions {
            name: "management",
            max_connections: 64,
            header_timeout: options.header_timeout,
            tls: None,
        };
        servers.spawn(async move {
            net::serve_connections(
                management,
                api::management_router(app.clone()),
                listen,
                &app.metrics,
                stop,
            )
            .await;
        });
    }

    let failure = tokio::select! {
        () = shutdown => {
            info!("shutdown requested");
            None
        }
        message = fatal => {
            error!(error = %message, "stopping");
            Some(message)
        }
    };

    app.set_ready(false);
    if let Some(on_stopping) = on_stopping {
        on_stopping();
    }
    let _ = stop_tx.send(true);
    let drained = async { while servers.join_next().await.is_some() {} };
    match tokio::time::timeout(options.grace, drained).await {
        Ok(()) => info!("stopped"),
        Err(_) => {
            warn!(
                grace_ms = options.grace.as_millis() as u64,
                "requests still running after the grace period; closing them"
            );
            servers.abort_all();
        }
    }
    failure.map_or(Ok(()), Err)
}

/// `SIGHUP` asks for a hot reload of the keys and profiles.
#[cfg(unix)]
async fn reload_on_sighup(reloader: reload::ReloadHandle) {
    use tokio::signal::unix::{SignalKind, signal};
    let Ok(mut hangup) = signal(SignalKind::hangup()) else {
        warn!("cannot listen for SIGHUP; reload by file change only");
        return;
    };
    while hangup.recv().await.is_some() {
        reloader.request("SIGHUP");
    }
}

/// Ctrl-C, and SIGTERM on Unix.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
