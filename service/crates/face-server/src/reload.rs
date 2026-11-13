//! Hot reload of the API keys and the profiles, without a restart and without dropping requests.
//!
//! Triggers: a change of the configuration file, `api_keys_file` or a profile file (polled every
//! `reload.watch_interval_s`), `SIGHUP` on Unix, and `sc.exe control <service> paramchange` on
//! Windows. A reload re-reads the configuration (with its environment overrides), the keys and
//! every profile, and checks them exactly as startup does. It is all-or-nothing: on any problem
//! the running set stays and the problems are logged. Requests in flight finish with the set
//! they started with. The limit state of unchanged keys (buckets, in-flight counts) carries over.
//!
//! Only keys and profiles are reloaded. Other settings (listener, paths to models and ONNX
//! Runtime, engine, logging, limits of the service itself) need a restart; a reload that sees
//! them changed says so in the log.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::api::{App, Catalog};
use crate::auth::{ApiKeys, now_unix};
use crate::config::Config;
use crate::profiles::ProfileStore;
use crate::tls::TlsState;

/// Asks the running service to reload (cheap to clone; requests are coalesced).
#[derive(Clone, Debug)]
pub struct ReloadHandle(mpsc::Sender<&'static str>);

impl ReloadHandle {
    /// Request a reload; `source` names the trigger in the log. Never blocks: when a reload is
    /// already queued, this one is merged into it.
    pub fn request(&self, source: &'static str) {
        let _ = self.0.try_send(source);
    }
}

/// A channel for reload requests: the handle for triggers and the receiver for [`run`].
pub fn channel() -> (ReloadHandle, mpsc::Receiver<&'static str>) {
    let (tx, rx) = mpsc::channel(1);
    (ReloadHandle(tx), rx)
}

/// What a successful reload loaded.
#[derive(Debug)]
pub struct Reloaded {
    pub profiles: usize,
    pub keys: usize,
    /// Settings that changed in the file but only apply after a restart.
    pub needs_restart: Vec<&'static str>,
    pub config: Config,
}

/// Reload the keys and profiles from `config_path` into `app`. Blocking (file I/O, hashing).
pub fn reload(
    app: &App,
    config_path: &Path,
    running: &Config,
    tls: Option<&TlsState>,
) -> Result<Reloaded, Vec<String>> {
    let config = Config::load(config_path).map_err(|e| e.problems)?;
    // The certificate and key, when the listener runs TLS (switching TLS on or off needs a
    // restart and is reported as such).
    let tls_config = match (tls, &config.server.tls) {
        (Some(_), Some(settings)) => {
            Some(crate::tls::server_config(settings).map_err(|e| vec![e])?)
        }
        _ => None,
    };
    let profiles = ProfileStore::load_dir(&config.paths.profiles)?;
    let previous = app.catalog();
    let keys = ApiKeys::from_config_with_state(&config.api_keys, &profiles, &previous.keys)?;
    let problems: Vec<String> = profiles
        .iter()
        .filter_map(|p| app.engine.check_profile(&p.profile).err())
        .collect();
    if !problems.is_empty() {
        return Err(problems);
    }
    let reloaded = Reloaded {
        profiles: profiles.len(),
        keys: keys.len(),
        needs_restart: restart_only_changes(running, &config),
        config,
    };
    for note in keys.validity_notes(now_unix()) {
        warn!("{note}");
    }
    app.replace_catalog(Catalog { profiles, keys })
        .map_err(|e| vec![e])?;
    if let (Some(state), Some(new)) = (tls, tls_config) {
        state.replace(new);
    }
    Ok(reloaded)
}

/// Settings that differ between the running and the reloaded configuration but are not
/// reloaded.
fn restart_only_changes(running: &Config, new: &Config) -> Vec<&'static str> {
    let mut changed = Vec::new();
    let mut check = |name: &'static str, same: bool| {
        if !same {
            changed.push(name);
        }
    };
    check(
        "server",
        format!("{:?}", running.server) == format!("{:?}", new.server),
    );
    check(
        "paths.models / paths.ort_library",
        running.paths.models == new.paths.models
            && running.paths.ort_library == new.paths.ort_library,
    );
    check(
        "engine",
        format!("{:?}", running.engine) == format!("{:?}", new.engine),
    );
    check(
        "logging",
        format!("{:?}", running.logging) == format!("{:?}", new.logging),
    );
    check("metrics", running.metrics.enabled == new.metrics.enabled);
    check(
        "health_requires_key",
        running.health_requires_key == new.health_requires_key,
    );
    check(
        "reload",
        running.reload.watch_interval_s == new.reload.watch_interval_s,
    );
    changed
}

/// The SHA-256 of every file a reload reads (the configuration, `api_keys_file`, the profile
/// files). Content hashes, not modification times: a same-size edit within the file system's
/// time resolution is still seen. The files are small, so hashing them each interval is cheap.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Fingerprint(Vec<(PathBuf, Option<[u8; 32]>)>);

impl Fingerprint {
    /// Take it *before* loading what it describes, so that a change in between causes one more
    /// reload rather than none.
    pub fn of(config_path: &Path, config: &Config) -> Fingerprint {
        let hash = |path: &Path| {
            let digest = std::fs::read(path)
                .ok()
                .map(|bytes| <[u8; 32]>::from(Sha256::digest(&bytes)));
            (path.to_path_buf(), digest)
        };
        let mut files = vec![hash(config_path)];
        if let Some(keys) = &config.api_keys_file {
            files.push(hash(keys));
        }
        if let Some(tls) = &config.server.tls {
            files.push(hash(&tls.cert));
            files.push(hash(&tls.key));
            if let Some(ca) = &tls.client_ca {
                files.push(hash(ca));
            }
        }
        if let Ok(entries) = std::fs::read_dir(&config.paths.profiles) {
            let mut profiles: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|e| {
                        e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml")
                    })
                })
                .collect();
            profiles.sort();
            files.extend(profiles.iter().map(|p| hash(p)));
        }
        Fingerprint(files)
    }
}

/// Serve reload requests and, when `watch_interval_s` > 0, poll the files for changes. Runs
/// until the request channel closes (the service stops).
pub async fn run(
    app: Arc<App>,
    config: Config,
    tls: Option<Arc<TlsState>>,
    initial: Fingerprint,
    mut requests: mpsc::Receiver<&'static str>,
) {
    let Some(config_path) = config.source.clone() else {
        // A configuration not loaded from a file (tests): nothing to reload from.
        while requests.recv().await.is_some() {}
        return;
    };
    let interval = (config.reload.watch_interval_s > 0)
        .then(|| Duration::from_secs(config.reload.watch_interval_s));
    let mut running = config;
    let mut seen = initial;
    let mut ticker = interval.map(tokio::time::interval);
    if let Some(t) = &mut ticker {
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        t.tick().await; // the first tick fires at once
    }
    loop {
        let source = tokio::select! {
            request = requests.recv() => match request {
                Some(source) => source,
                None => return,
            },
            () = async {
                match &mut ticker {
                    Some(t) => { t.tick().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                let (path, cfg) = (config_path.clone(), running.clone());
                let now = tokio::task::spawn_blocking(move || Fingerprint::of(&path, &cfg))
                    .await
                    .unwrap_or_default();
                if now == seen {
                    continue;
                }
                seen = now;
                "file change"
            }
        };
        let (worker_app, path, cfg, worker_tls) = (
            app.clone(),
            config_path.clone(),
            running.clone(),
            tls.clone(),
        );
        let result = tokio::task::spawn_blocking(move || {
            reload(&worker_app, &path, &cfg, worker_tls.as_deref())
        })
        .await;
        match result {
            Ok(Ok(reloaded)) => {
                app.metrics.config_reloads.with_label_values(&["ok"]).inc();
                info!(
                    trigger = source,
                    profiles = reloaded.profiles,
                    api_keys = reloaded.keys,
                    "reloaded API keys and profiles"
                );
                if !reloaded.needs_restart.is_empty() {
                    warn!(
                        settings = %reloaded.needs_restart.join(", "),
                        "changed settings take effect only after a restart"
                    );
                }
                running = reloaded.config;
                let (path, cfg) = (config_path.clone(), running.clone());
                seen = tokio::task::spawn_blocking(move || Fingerprint::of(&path, &cfg))
                    .await
                    .unwrap_or_default();
            }
            Ok(Err(problems)) => {
                app.metrics
                    .config_reloads
                    .with_label_values(&["error"])
                    .inc();
                error!(
                    trigger = source,
                    problems = %problems.join("; "),
                    "reload refused; the running API keys and profiles stay in force"
                );
            }
            Err(_) => {
                app.metrics
                    .config_reloads
                    .with_label_values(&["error"])
                    .inc();
                error!(
                    trigger = source,
                    "reload panicked; the running set stays in force"
                );
            }
        }
    }
}
