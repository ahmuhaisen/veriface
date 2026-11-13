//! The TOML configuration file. See `service/config.example.toml`.
//!
//! Relative paths are resolved against the directory of the configuration file, so the service
//! behaves the same whatever its working directory (a Windows service starts in `System32`).
//!
//! Two additions for containers and secret stores:
//! - **Environment overrides.** `VERIFACE__<TABLE>__<KEY>=value` replaces a key of the file,
//!   e.g. `VERIFACE__SERVER__BIND=0.0.0.0:8080` (see [`EnvOverride`]).
//! - **`api_keys_file`.** The `[[api_keys]]` entries may also live in a separate TOML file (a
//!   mounted secret); they are added to the entries of the main file.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use serde::Deserialize;
use tracing_subscriber::filter::Targets;

/// One or more problems that stop the service from starting. All of them are reported at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupError {
    pub problems: Vec<String>,
}

impl StartupError {
    pub fn new(problem: impl Into<String>) -> Self {
        StartupError {
            problems: vec![problem.into()],
        }
    }

    pub fn from_problems(problems: Vec<String>) -> Result<(), StartupError> {
        if problems.is_empty() {
            Ok(())
        } else {
            Err(StartupError { problems })
        }
    }
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.problems.as_slice() {
            [one] => write!(f, "{one}"),
            many => {
                write!(f, "{} problems:", many.len())?;
                for p in many {
                    write!(f, "\n  - {p}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for StartupError {}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Require a valid `X-API-Key` on `/health/live` and `/health/ready` too.
    #[serde(default)]
    pub health_requires_key: bool,
    #[serde(default)]
    pub server: ServerConfig,
    pub paths: PathsConfig,
    #[serde(default)]
    pub api_keys: Vec<ApiKeyConfig>,
    /// A TOML file holding more `[[api_keys]]` entries (and nothing else), e.g. a mounted secret.
    #[serde(default)]
    pub api_keys_file: Option<PathBuf>,
    #[serde(default)]
    pub metrics: MetricsConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub engine: EngineConfig,
    #[serde(default)]
    pub reload: ReloadConfig,
    /// Directory of the configuration file (relative paths are resolved against it).
    #[serde(skip)]
    pub base_dir: PathBuf,
    /// The file this configuration was loaded from ([`Config::load`] only); hot reload re-reads
    /// and watches it.
    #[serde(skip)]
    pub source: Option<PathBuf>,
}

/// Hot reload of the API keys and the profiles (see `crate::reload`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ReloadConfig {
    /// Check the configuration file, `api_keys_file` and the profiles directory for changes
    /// every this many seconds, and reload when one changed. 0 = only on request (SIGHUP on
    /// Linux, `sc.exe control <service> paramchange` on Windows).
    pub watch_interval_s: u64,
}

impl Default for ReloadConfig {
    fn default() -> Self {
        ReloadConfig {
            watch_interval_s: 10,
        }
    }
}

/// Which engine runs, and its ONNX Runtime settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EngineConfig {
    /// `pipeline` (the vision models) or `stub` (no models; every check undetermined; for
    /// tests and smoke runs only).
    pub kind: EngineKind,
    /// Threads ONNX Runtime may use inside one inference.
    pub intra_op_threads: usize,
    /// Per-model overrides of `intra_op_threads`, by manifest model id. Default:
    /// [`EngineConfig::DEFAULT_THREAD_OVERRIDES`]; a table in the file replaces it entirely (an
    /// empty table turns the overrides off).
    pub intra_op_threads_per_model: BTreeMap<String, usize>,
    /// ORT sessions per model (each runs one inference at a time). 0 = `max_concurrency`, capped
    /// at [`EngineConfig::DEFAULT_SESSION_CAP`]. Every session holds its own copy of the weights
    /// (about 290 MB per set, mostly AuraFace).
    pub sessions_per_model: usize,
    /// Optional pin: hex SHA-256 the ONNX Runtime library must have.
    pub onnxruntime_sha256: Option<String>,
    /// Run a verification's independent stages concurrently (probe analysis with reference
    /// extraction; the two embeddings on two sessions). Same results, lower latency; a
    /// verification then uses up to two cores.
    pub parallel_stages: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            kind: EngineKind::Pipeline,
            intra_op_threads: 1,
            intra_op_threads_per_model: EngineConfig::DEFAULT_THREAD_OVERRIDES
                .iter()
                .map(|(id, threads)| ((*id).to_owned(), *threads))
                .collect(),
            sessions_per_model: 0,
            onnxruntime_sha256: None,
            parallel_stages: true,
        }
    }
}

impl EngineConfig {
    /// 2 intra-op threads for the two heaviest models. Measured (release, 16 cores): a genuine
    /// verification 945-1087 ms -> 700-782 ms, quality ~330 -> ~250 ms; 16 concurrent
    /// verifications 3.75-4.50 -> 4.46-4.67 req/s; scores identical on the 12-pair fixture set.
    pub const DEFAULT_THREAD_OVERRIDES: &'static [(&'static str, usize)] =
        &[("auraface-v1", 2), ("mp-selfie-multiclass", 2)];

    /// Measured on 16 cores with 16 concurrent verifications: 4 sessions per model gave 3.9
    /// requests/s with 1.2 GB; 16 sessions gave 3.6 requests/s with 4.7 GB. Most of a request's
    /// time is spent outside the model sessions, so more sessions only cost memory.
    pub const DEFAULT_SESSION_CAP: usize = 4;

    /// `sessions_per_model`, with 0 replaced by `min(max_concurrency, DEFAULT_SESSION_CAP)`.
    pub fn effective_sessions(&self, max_concurrency: usize) -> usize {
        match self.sessions_per_model {
            0 => max_concurrency.clamp(1, Self::DEFAULT_SESSION_CAP),
            n => n,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    #[default]
    Pipeline,
    Stub,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    /// `ip:port` to listen on.
    pub bind: String,
    /// Requests processed at once (CPU work). 0 = the machine's available parallelism.
    pub max_concurrency: usize,
    /// Requests allowed to wait for a free slot; beyond this, `503 overloaded`.
    pub queue_limit: usize,
    /// Whole-request deadline (queueing, upload and processing), in milliseconds.
    pub request_timeout_ms: u64,
    /// Optional second listener, plain HTTP, serving only `/health/live`, `/health/ready` and
    /// `/metrics` (for probes and monitoring; e.g. `127.0.0.1:9090`). When set, `/metrics` is no
    /// longer served on `bind`.
    pub management_bind: Option<String>,
    /// Open connections accepted at once (per listener); beyond this new connections wait in
    /// the operating system's backlog.
    pub max_connections: usize,
    /// Time a client has to send a request's headers (and, with TLS, to finish the handshake),
    /// in milliseconds. Bounds slow-header ("slowloris") connections.
    pub header_timeout_ms: u64,
    /// Reverse proxies (IP addresses or CIDR ranges) whose `X-Forwarded-For` is trusted for the
    /// client address in the logs. Empty: the peer address is the client.
    pub trusted_proxies: Vec<String>,
    /// In-process TLS on `bind` (and mTLS with `client_ca`).
    pub tls: Option<TlsConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            bind: "127.0.0.1:8080".to_owned(),
            max_concurrency: 0,
            queue_limit: 32,
            request_timeout_ms: 30_000,
            management_bind: None,
            max_connections: 1024,
            header_timeout_ms: 10_000,
            trusted_proxies: Vec::new(),
            tls: None,
        }
    }
}

/// `[server.tls]`. PEM files; relative paths are resolved against the configuration file. The
/// certificate and key are re-read on every hot reload (certificate renewal without restart).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    /// The server certificate chain (leaf first).
    pub cert: PathBuf,
    /// The private key (PKCS#8, PKCS#1 or SEC1).
    pub key: PathBuf,
    /// CA certificates that sign client certificates. Set: mutual TLS.
    #[serde(default)]
    pub client_ca: Option<PathBuf>,
    /// With `client_ca`: `required` (default; no certificate, no connection) or `optional`.
    #[serde(default)]
    pub client_auth: ClientAuth,
    /// Lowest protocol version: `1.2` (default) or `1.3`.
    #[serde(default = "TlsConfig::default_min_version")]
    pub min_version: String,
}

impl TlsConfig {
    fn default_min_version() -> String {
        "1.2".to_owned()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientAuth {
    #[default]
    Required,
    Optional,
}

/// An IP address or CIDR range (`10.0.0.0/8`, `fd00::/8`, `192.0.2.7`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpRange {
    network: std::net::IpAddr,
    prefix: u8,
}

impl IpRange {
    pub fn parse(text: &str) -> Result<IpRange, String> {
        let bad = || format!("`{text}` is not an IP address or CIDR range");
        let (addr, prefix) = match text.split_once('/') {
            Some((a, p)) => (a, Some(p.parse::<u8>().map_err(|_| bad())?)),
            None => (text, None),
        };
        let network: std::net::IpAddr = addr.trim().parse().map_err(|_| bad())?;
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        if prefix > max {
            return Err(bad());
        }
        Ok(IpRange { network, prefix })
    }

    pub fn contains(&self, ip: std::net::IpAddr) -> bool {
        use std::net::IpAddr;
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
            v4 => v4,
        };
        match (self.network, ip) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(n) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(n) & mask == u128::from(a) & mask
            }
            _ => false,
        }
    }
}

impl ServerConfig {
    /// `max_concurrency`, with 0 replaced by the available parallelism.
    pub fn effective_concurrency(&self) -> usize {
        if self.max_concurrency > 0 {
            self.max_concurrency
        } else {
            std::thread::available_parallelism().map_or(1, |n| n.get())
        }
    }

    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms)
    }

    pub fn bind_addr(&self) -> Result<SocketAddr, String> {
        self.bind.parse().map_err(|_| {
            format!(
                "server.bind `{}` is not an `ip:port` address (e.g. 0.0.0.0:8080)",
                self.bind
            )
        })
    }

    pub fn management_addr(&self) -> Result<Option<SocketAddr>, String> {
        self.management_bind
            .as_deref()
            .map(|bind| {
                bind.parse().map_err(|_| {
                    format!(
                        "server.management_bind `{bind}` is not an `ip:port` address (e.g. 127.0.0.1:9090)"
                    )
                })
            })
            .transpose()
    }

    pub fn header_timeout(&self) -> Duration {
        Duration::from_millis(self.header_timeout_ms)
    }

    pub fn trusted_proxy_ranges(&self) -> Result<Vec<IpRange>, String> {
        self.trusted_proxies
            .iter()
            .map(|p| IpRange::parse(p).map_err(|e| format!("server.trusted_proxies: {e}")))
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathsConfig {
    /// Directory holding `manifest.json` and the model files.
    pub models: PathBuf,
    /// Directory of profile files (`*.yaml`).
    pub profiles: PathBuf,
    /// The ONNX Runtime library (`onnxruntime.dll` / `libonnxruntime.so`).
    #[serde(default)]
    pub ort_library: Option<PathBuf>,
}

/// One `[[api_keys]]` entry. Only the SHA-256 of the key is stored.
///
/// A project may hold several keys (rotation: add the new key, move the clients, then remove the
/// old one or let it `expire`); `label` tells them apart in the logs.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyConfig {
    /// Integrating project, for logs and metrics.
    pub project: String,
    /// Optional name of this key, unique among the keys (e.g. `crm-2026-10`); logged.
    #[serde(default)]
    pub label: Option<String>,
    /// Lower- or upper-case hex SHA-256 of the key (UTF-8 bytes as sent in `X-API-Key`).
    pub sha256: String,
    /// Profiles this key may use. `"*"` allows every loaded profile.
    pub profiles: Vec<String>,
    /// Used when a request names no profile. Defaults to `standard`.
    #[serde(default)]
    pub default_profile: Option<String>,
    /// Requests per minute this key may make (token bucket); none = no limit.
    #[serde(default)]
    pub rate_per_minute: Option<u32>,
    /// Bucket size: requests that may arrive at once. Default `max(1, rate_per_minute / 10)`.
    #[serde(default)]
    pub burst: Option<u32>,
    /// Requests of this key in flight at once; none = no limit (beyond the service's own).
    #[serde(default)]
    pub max_concurrency: Option<u32>,
    /// The key is refused (401) before this TOML date or date-time (a date means 00:00 UTC; a
    /// date-time without offset is UTC).
    #[serde(default)]
    pub not_before: Option<toml::value::Datetime>,
    /// The key is refused (401) from this TOML date or date-time on.
    #[serde(default)]
    pub expires: Option<toml::value::Datetime>,
}

/// Seconds since the Unix epoch of a TOML date or date-time (UTC when it has no offset).
pub fn unix_seconds(value: &toml::value::Datetime) -> Result<i64, String> {
    let Some(date) = value.date else {
        return Err(format!("`{value}` is a time without a date"));
    };
    let (y, m, d) = (
        i64::from(date.year),
        i64::from(date.month),
        i64::from(date.day),
    );
    // Days from civil (Howard Hinnant's algorithm), proleptic Gregorian.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut seconds = days * 86_400;
    if let Some(time) = value.time {
        seconds += i64::from(time.hour) * 3600
            + i64::from(time.minute) * 60
            + i64::from(time.second.unwrap_or(0));
    }
    if let Some(toml::value::Offset::Custom { minutes }) = value.offset {
        seconds -= i64::from(minutes) * 60;
    }
    Ok(seconds)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MetricsConfig {
    /// Serve `GET /metrics` (Prometheus text format, no authentication).
    pub enabled: bool,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        MetricsConfig { enabled: true }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Json,
    Pretty,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LoggingConfig {
    pub format: LogFormat,
    /// `tracing` filter directives, e.g. `info` or `info,face_server=debug`.
    pub level: String,
    /// Append logs to this file instead of stdout (needed when running as a Windows service).
    pub file: Option<PathBuf>,
    /// Rotate `file` when it would grow beyond this many MiB (0 = never rotate).
    pub max_file_mb: u64,
    /// Rotated files kept: `<file>.1` (newest) to `<file>.<max_files>`.
    pub max_files: usize,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        LoggingConfig {
            format: LogFormat::Json,
            level: "info".to_owned(),
            file: None,
            max_file_mb: 100,
            max_files: 10,
        }
    }
}

impl LoggingConfig {
    pub fn filter(&self) -> Result<Targets, String> {
        Targets::from_str(&self.level)
            .map_err(|e| format!("logging.level `{}` is invalid: {e}", self.level))
    }
}

/// Upper bound for `request_timeout_ms` (10 minutes).
const MAX_TIMEOUT_MS: u64 = 600_000;

impl Config {
    /// Read, parse, apply the `VERIFACE__*` environment overrides, resolve relative paths,
    /// read `api_keys_file` and validate. Profiles are loaded separately.
    pub fn load(path: &Path) -> Result<Config, StartupError> {
        Config::load_with_env(path, std::env::vars_os())
    }

    /// [`Config::load`] with the environment given explicitly.
    pub fn load_with_env<K, V>(
        path: &Path,
        env: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Config, StartupError>
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let text = std::fs::read_to_string(path).map_err(|e| {
            StartupError::new(format!("cannot read config `{}`: {e}", path.display()))
        })?;
        let base = std::path::absolute(path)
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let overrides = EnvOverride::from_env(env)?;
        let mut config = Config::from_toml_with(&text, &base, &overrides).map_err(|mut e| {
            for p in &mut e.problems {
                *p = format!("{}: {p}", path.display());
            }
            e
        })?;
        config.source = Some(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
        Ok(config)
    }

    /// Parse and validate TOML; relative paths are resolved against `base`.
    pub fn from_toml(text: &str, base: &Path) -> Result<Config, StartupError> {
        Config::from_toml_with(text, base, &[])
    }

    /// [`Config::from_toml`] with environment overrides applied on top of the file.
    pub fn from_toml_with(
        text: &str,
        base: &Path,
        overrides: &[EnvOverride],
    ) -> Result<Config, StartupError> {
        let mut config = Config::parse(text, overrides)?;
        config.resolve_paths(base);
        if let Some(file) = &config.api_keys_file {
            let mut extra =
                read_api_keys_file(file).map_err(|problems| StartupError { problems })?;
            config.api_keys.append(&mut extra);
        }
        config.validate()?;
        Ok(config)
    }

    fn parse(text: &str, overrides: &[EnvOverride]) -> Result<Config, StartupError> {
        let syntax = |e: toml::de::Error| StartupError::new(e.to_string().trim().to_owned());
        if overrides.is_empty() {
            // Straight from the text, so that errors keep their line and column.
            return toml::from_str(text).map_err(syntax);
        }
        let mut table: toml::Table = toml::from_str(text).map_err(syntax)?;
        let problems: Vec<String> = overrides
            .iter()
            .filter_map(|o| o.apply(&mut table).err())
            .collect();
        StartupError::from_problems(problems)?;
        table.try_into().map_err(|e: toml::de::Error| {
            let names: Vec<&str> = overrides.iter().map(|o| o.var.as_str()).collect();
            StartupError::new(format!(
                "{} (with the environment overrides {})",
                e.to_string().trim(),
                names.join(", ")
            ))
        })
    }

    fn resolve_paths(&mut self, base: &Path) {
        self.base_dir = base.to_path_buf();
        let resolve = |p: &mut PathBuf| {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        };
        resolve(&mut self.paths.models);
        resolve(&mut self.paths.profiles);
        if let Some(p) = &mut self.paths.ort_library {
            resolve(p);
        }
        if let Some(p) = &mut self.logging.file {
            resolve(p);
        }
        if let Some(p) = &mut self.api_keys_file {
            resolve(p);
        }
        if let Some(tls) = &mut self.server.tls {
            resolve(&mut tls.cert);
            resolve(&mut tls.key);
            if let Some(ca) = &mut tls.client_ca {
                resolve(ca);
            }
        }
    }

    /// Everything that can be checked without reading other files.
    fn validate(&self) -> Result<(), StartupError> {
        let mut problems = Vec::new();
        if let Err(e) = self.server.bind_addr() {
            problems.push(e);
        }
        if self.server.request_timeout_ms == 0 || self.server.request_timeout_ms > MAX_TIMEOUT_MS {
            problems.push(format!(
                "server.request_timeout_ms must be within 1-{MAX_TIMEOUT_MS}"
            ));
        }
        if self.server.max_concurrency > 4096 {
            problems.push("server.max_concurrency must be at most 4096".to_owned());
        }
        match self.server.management_addr() {
            Ok(Some(management)) if self.server.bind_addr() == Ok(management) => {
                problems.push("server.management_bind must differ from server.bind".to_owned())
            }
            Ok(_) => {}
            Err(e) => problems.push(e),
        }
        if !(1..=1_000_000).contains(&self.server.max_connections) {
            problems.push("server.max_connections must be within 1-1000000".to_owned());
        }
        if !(1_000..=MAX_TIMEOUT_MS).contains(&self.server.header_timeout_ms) {
            problems.push(format!(
                "server.header_timeout_ms must be within 1000-{MAX_TIMEOUT_MS}"
            ));
        }
        if let Err(e) = self.server.trusted_proxy_ranges() {
            problems.push(e);
        }
        if let Some(tls) = &self.server.tls
            && !matches!(tls.min_version.as_str(), "1.2" | "1.3")
        {
            problems.push("server.tls.min_version must be \"1.2\" or \"1.3\"".to_owned());
        }
        if !(1..=64).contains(&self.engine.intra_op_threads) {
            problems.push("engine.intra_op_threads must be within 1-64".to_owned());
        }
        for (model, threads) in &self.engine.intra_op_threads_per_model {
            if !(1..=64).contains(threads) {
                problems.push(format!(
                    "engine.intra_op_threads_per_model.{model} must be within 1-64"
                ));
            }
        }
        if self.engine.sessions_per_model > 256 {
            problems.push("engine.sessions_per_model must be at most 256".to_owned());
        }
        if let Some(pin) = &self.engine.onnxruntime_sha256
            && crate::auth::parse_sha256_hex(pin).is_none()
        {
            problems.push("engine.onnxruntime_sha256 must be 64 hex characters".to_owned());
        }
        if let Err(e) = self.logging.filter() {
            problems.push(e);
        }
        if !(1..=1000).contains(&self.logging.max_files) {
            problems.push("logging.max_files must be within 1-1000".to_owned());
        }
        if self.logging.max_file_mb > 1024 * 1024 {
            problems.push("logging.max_file_mb must be at most 1048576 (1 TiB)".to_owned());
        }

        if self.api_keys.is_empty() {
            problems.push(
                "no [[api_keys]] configured: every /v1 call needs a key, so the service would be unusable"
                    .to_owned(),
            );
        }
        let mut labels = HashSet::new();
        let mut digests = HashSet::new();
        for (i, key) in self.api_keys.iter().enumerate() {
            let at = format!("api_keys[{i}] ({})", key.project);
            if key.project.trim().is_empty() {
                problems.push(format!("api_keys[{i}]: project must not be empty"));
            }
            if let Some(label) = &key.label
                && (label.trim().is_empty() || !labels.insert(label.as_str()))
            {
                problems.push(format!("{at}: label `{label}` is empty or used twice"));
            }
            for (name, value) in [
                ("rate_per_minute", key.rate_per_minute),
                ("burst", key.burst),
                ("max_concurrency", key.max_concurrency),
            ] {
                if value == Some(0) {
                    problems.push(format!(
                        "{at}: {name} must be at least 1 (omit it for no limit)"
                    ));
                }
            }
            if key.burst.is_some() && key.rate_per_minute.is_none() {
                problems.push(format!("{at}: burst needs rate_per_minute"));
            }
            let window: Vec<Option<i64>> = [&key.not_before, &key.expires]
                .into_iter()
                .map(|v| match v.as_ref().map(unix_seconds) {
                    Some(Ok(t)) => Some(t),
                    Some(Err(e)) => {
                        problems.push(format!("{at}: {e}"));
                        None
                    }
                    None => None,
                })
                .collect();
            if let [Some(from), Some(until)] = window[..]
                && from >= until
            {
                problems.push(format!("{at}: not_before must be earlier than expires"));
            }
            match crate::auth::parse_sha256_hex(&key.sha256) {
                Some(digest) => {
                    if !digests.insert(digest) {
                        problems.push(format!("{at}: the same key hash is configured twice"));
                    }
                }
                None => problems.push(format!(
                    "{at}: sha256 must be 64 hex characters (the SHA-256 of the key, not the key)"
                )),
            }
            if key.profiles.is_empty() {
                problems.push(format!(
                    "{at}: profiles must list at least one profile (or \"*\")"
                ));
            }
        }
        StartupError::from_problems(problems)
    }
}

/// The contents of an `api_keys_file`: `[[api_keys]]` entries only.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiKeysFile {
    #[serde(default)]
    api_keys: Vec<ApiKeyConfig>,
}

/// Read the `[[api_keys]]` entries of a keys file (see `api_keys_file`).
pub fn read_api_keys_file(path: &Path) -> Result<Vec<ApiKeyConfig>, Vec<String>> {
    let at = format!("api_keys_file `{}`", path.display());
    let text = std::fs::read_to_string(path).map_err(|e| vec![format!("cannot read {at}: {e}")])?;
    toml::from_str::<ApiKeysFile>(&text)
        .map(|file| file.api_keys)
        .map_err(|e| vec![format!("{at}: {}", e.to_string().trim())])
}

/// Prefix of the environment variables that override configuration keys.
pub const ENV_PREFIX: &str = "VERIFACE__";

/// One `VERIFACE__<TABLE>__<KEY>=value` environment variable.
///
/// - The name after the prefix is the key path with `__` between levels, case-insensitive:
///   `VERIFACE__SERVER__BIND`, `VERIFACE__PATHS__MODELS`, `VERIFACE__API_KEYS_FILE`.
/// - The value is a TOML boolean, number or array when it reads as one (`true`, `8`,
///   `["a", "b"]`), otherwise the plain string (`0.0.0.0:8080`, `info,face_server=debug`).
/// - An empty value removes the key: `VERIFACE__LOGGING__FILE=` logs to stdout.
/// - `[[api_keys]]` cannot be set this way; point `VERIFACE__API_KEYS_FILE` at a keys file.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvOverride {
    /// The variable's name, for messages.
    pub var: String,
    path: Vec<String>,
    /// `None` removes the key.
    value: Option<toml::Value>,
}

impl EnvOverride {
    /// Every `VERIFACE__*` variable, in name order. Reports every malformed one.
    pub fn from_env<K, V>(env: impl IntoIterator<Item = (K, V)>) -> Result<Vec<Self>, StartupError>
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        let mut overrides = Vec::new();
        let mut problems = Vec::new();
        for (name, value) in env {
            let Some(name) = name.as_ref().to_str() else {
                continue;
            };
            if !name.to_ascii_uppercase().starts_with(ENV_PREFIX) {
                continue;
            }
            let parsed = match value.as_ref().to_str() {
                Some(value) => EnvOverride::parse(name, value),
                None => Err(format!(
                    "environment variable {name}: the value is not valid UTF-8"
                )),
            };
            match parsed {
                Ok(o) => overrides.push(o),
                Err(e) => problems.push(e),
            }
        }
        StartupError::from_problems(problems)?;
        overrides.sort_by(|a, b| a.var.cmp(&b.var));
        Ok(overrides)
    }

    /// One variable; `name` includes the prefix (in any case).
    pub fn parse(name: &str, raw: &str) -> Result<Self, String> {
        let at = format!("environment variable {name}");
        let path: Vec<String> = name
            .get(ENV_PREFIX.len()..)
            .unwrap_or_default()
            .split("__")
            .map(str::to_ascii_lowercase)
            .collect();
        let valid =
            |s: &String| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !path.iter().all(valid) {
            return Err(format!(
                "{at}: expected VERIFACE__<KEY> or VERIFACE__<TABLE>__<KEY> \
                 (letters, digits and `_`; `__` between levels)"
            ));
        }
        if path[0] == "api_keys" {
            return Err(format!(
                "{at}: [[api_keys]] cannot be set from the environment; \
                 set VERIFACE__API_KEYS_FILE to a keys file instead"
            ));
        }
        Ok(EnvOverride {
            var: name.to_owned(),
            path,
            value: (!raw.is_empty()).then(|| EnvOverride::value(raw)),
        })
    }

    /// A TOML boolean, number or array when `raw` reads as one, else the string itself.
    fn value(raw: &str) -> toml::Value {
        let parsed = toml::from_str::<toml::Table>(&format!("v = {raw}"))
            .ok()
            .and_then(|mut t| t.remove("v"));
        match parsed {
            Some(
                v @ (toml::Value::Boolean(_)
                | toml::Value::Integer(_)
                | toml::Value::Float(_)
                | toml::Value::Array(_)),
            ) => v,
            _ => toml::Value::String(raw.to_owned()),
        }
    }

    fn apply(&self, root: &mut toml::Table) -> Result<(), String> {
        let Some((key, tables)) = self.path.split_last() else {
            return Ok(());
        };
        let mut table = root;
        for (depth, name) in tables.iter().enumerate() {
            let entry = table
                .entry(name.clone())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()));
            table = match entry {
                toml::Value::Table(t) => t,
                _ => {
                    return Err(format!(
                        "environment variable {}: `{}` is not a table",
                        self.var,
                        self.path[..=depth].join(".")
                    ));
                }
            };
        }
        match &self.value {
            Some(v) => {
                table.insert(key.clone(), v.clone());
            }
            None => {
                table.remove(key);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> Vec<EnvOverride> {
        EnvOverride::from_env(vars.iter().copied()).unwrap()
    }

    #[test]
    fn environment_overrides_replace_add_and_remove_keys() {
        let text = format!("{MINIMAL}\n[logging]\nfile = \"logs/x.log\"\n");
        let overrides = env(&[
            ("VERIFACE__SERVER__BIND", "0.0.0.0:9000"),
            ("veriface__server__queue_limit", "7"),
            ("VERIFACE__PATHS__MODELS", "pack"),
            ("VERIFACE__LOGGING__FILE", ""),
            ("VERIFACE__HEALTH_REQUIRES_KEY", "true"),
            ("VERIFACE__ENGINE__INTRA_OP_THREADS", "2"),
            ("PATH", "ignored"),
        ]);
        assert_eq!(overrides.len(), 6);
        let base = Path::new("/opt/x");
        let c = Config::from_toml_with(&text, base, &overrides).unwrap();
        assert_eq!(c.server.bind, "0.0.0.0:9000");
        assert_eq!(c.server.queue_limit, 7);
        assert_eq!(c.paths.models, base.join("pack"));
        assert_eq!(c.paths.profiles, base.join("../profiles"));
        assert_eq!(c.logging.file, None);
        assert!(c.health_requires_key);
        assert_eq!(c.engine.intra_op_threads, 2);
    }

    #[test]
    fn environment_override_problems_are_reported() {
        assert!(EnvOverride::parse("VERIFACE__", "x").is_err());
        assert!(EnvOverride::parse("VERIFACE__SERVER____BIND", "x").is_err());
        assert!(EnvOverride::parse("VERIFACE__API_KEYS", "x").is_err());
        let e = EnvOverride::from_env([("VERIFACE__A-B", "1"), ("VERIFACE__API_KEYS", "x")])
            .unwrap_err();
        assert_eq!(e.problems.len(), 2, "{e}");
        // An unknown key is still an error, and the message names the variables.
        let typo = env(&[("VERIFACE__SERVER__BINDD", "0.0.0.0:1")]);
        let e = Config::from_toml_with(MINIMAL, Path::new("."), &typo).unwrap_err();
        assert!(e.to_string().contains("VERIFACE__SERVER__BINDD"), "{e}");
        // A scalar where a table is needed.
        let clash = env(&[("VERIFACE__HEALTH_REQUIRES_KEY__X", "1")]);
        let text = format!("health_requires_key = true\n{MINIMAL}");
        let e = Config::from_toml_with(&text, Path::new("."), &clash).unwrap_err();
        assert!(e.to_string().contains("is not a table"), "{e}");
    }

    #[test]
    fn environment_values_are_typed_only_when_unambiguous() {
        let v = |raw| EnvOverride::parse("VERIFACE__X", raw).unwrap().value;
        let s = |raw: &str| Some(toml::Value::String(raw.to_owned()));
        assert_eq!(v("8"), Some(toml::Value::Integer(8)));
        assert_eq!(v("false"), Some(toml::Value::Boolean(false)));
        assert_eq!(v("info,face_server=debug"), s("info,face_server=debug"));
        assert_eq!(v("127.0.0.1:8080"), s("127.0.0.1:8080"));
        assert_eq!(v("1979-05-27"), s("1979-05-27"));
        assert_eq!(v("/run/secrets/keys.toml"), s("/run/secrets/keys.toml"));
        assert!(matches!(v(r#"["a", "b"]"#), Some(toml::Value::Array(_))));
        assert_eq!(v(""), None);
    }

    #[test]
    fn api_keys_file_adds_keys() {
        let dir = std::env::temp_dir().join(format!("veriface-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let keys = dir.join("keys.toml");
        let entry = |project: &str, sha: &str| {
            format!(
                "[[api_keys]]\nproject = \"{project}\"\nsha256 = \"{sha}\"\nprofiles = [\"*\"]\n"
            )
        };
        let other = "486ea46224d1bb4fb680f34f7c9ad96a8f24ec88be73ea8e5a6c65260e9cb8a7";
        std::fs::write(&keys, entry("other", other)).unwrap();
        let text = format!("api_keys_file = \"keys.toml\"\n{MINIMAL}");
        let c = Config::from_toml(&text, &dir).unwrap();
        assert_eq!(c.api_keys.len(), 2);
        assert_eq!(c.api_keys[1].project, "other");
        assert_eq!(c.api_keys_file, Some(dir.join("keys.toml")));

        // Keys only in the file.
        let only = "api_keys_file = \"keys.toml\"\n[paths]\nmodels = \"m\"\nprofiles = \"p\"\n";
        assert_eq!(Config::from_toml(only, &dir).unwrap().api_keys.len(), 1);
        // The same hash in both places is a duplicate.
        let same = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        std::fs::write(&keys, entry("dup", same)).unwrap();
        assert!(Config::from_toml(&text, &dir).is_err());
        // Anything but [[api_keys]] is refused.
        std::fs::write(&keys, "[server]\nbind = \"0.0.0.0:1\"\n").unwrap();
        assert!(Config::from_toml(&text, &dir).is_err());
        // A missing file is a startup problem.
        std::fs::remove_file(&keys).unwrap();
        let e = Config::from_toml(&text, &dir).unwrap_err();
        assert!(e.to_string().contains("api_keys_file"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const MINIMAL: &str = r#"
[paths]
models = "models"
profiles = "../profiles"

[[api_keys]]
project = "demo"
sha256 = "2CF24DBA5FB0A30E26E83B2AC5B9E29E1B161E5C1FA7425E73043362938B9824"
profiles = ["standard"]
"#;

    #[test]
    fn defaults_and_relative_paths() {
        let base = Path::new("/opt/veriface");
        let c = Config::from_toml(MINIMAL, base).unwrap();
        assert_eq!(c.server.bind, "127.0.0.1:8080");
        assert_eq!(c.server.queue_limit, 32);
        assert!(c.server.effective_concurrency() >= 1);
        assert!(c.metrics.enabled);
        assert!(!c.health_requires_key);
        assert_eq!(c.logging.format, LogFormat::Json);
        assert_eq!(c.paths.models, base.join("models"));
        assert_eq!(c.paths.profiles, base.join("../profiles"));
        assert_eq!(c.engine.kind, EngineKind::Pipeline);
        assert_eq!(c.engine.intra_op_threads, 1);
        assert_eq!(
            c.engine.intra_op_threads_per_model.get("auraface-v1"),
            Some(&2)
        );
        assert_eq!(c.engine.intra_op_threads_per_model.len(), 2);
        assert_eq!(c.engine.effective_sessions(16), 4);
        assert_eq!(c.engine.effective_sessions(2), 2);
        let stub = MINIMAL.replace(
            "[paths]",
            "[engine]
kind = \"stub\"
sessions_per_model = 9

[paths]",
        );
        let c = Config::from_toml(&stub, base).unwrap();
        assert_eq!(c.engine.kind, EngineKind::Stub);
        assert_eq!(c.engine.effective_sessions(16), 9);
        // An explicit (empty) table replaces the default overrides.
        let off = MINIMAL.replace(
            "[paths]",
            "[engine.intra_op_threads_per_model]

[paths]",
        );
        let c = Config::from_toml(&off, base).unwrap();
        assert!(c.engine.intra_op_threads_per_model.is_empty());
        let bad = MINIMAL.replace(
            "[paths]",
            "[engine.intra_op_threads_per_model]
\"auraface-v1\" = 0

[paths]",
        );
        assert!(Config::from_toml(&bad, base).is_err());
    }

    #[test]
    fn reports_every_problem_at_once() {
        let text = r#"
[server]
bind = "localhost"
request_timeout_ms = 0

[paths]
models = "m"
profiles = "p"

[logging]
level = "info,face_server=loud"

[[api_keys]]
project = "a"
sha256 = "abc"
profiles = []

[[api_keys]]
project = "a"
sha256 = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
profiles = ["standard"]
"#;
        let e = Config::from_toml(text, Path::new(".")).unwrap_err();
        // bind, timeout, level, sha256, empty profiles (a project may hold several keys).
        assert_eq!(e.problems.len(), 5, "{e}");
    }

    #[test]
    fn example_config_is_valid_apart_from_the_key_placeholder() {
        let example = include_str!("../../../config.example.toml");
        let e = Config::from_toml(example, Path::new("/opt/veriface")).unwrap_err();
        assert_eq!(e.problems.len(), 1, "{e}");
        assert!(e.problems[0].contains("sha256"), "{e}");

        let filled = example.replace(
            "REPLACE-WITH-THE-SHA256-HEX-OF-THE-KEY",
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        );
        let c = Config::from_toml(&filled, Path::new("/opt/veriface")).unwrap();
        assert_eq!(c.api_keys[0].default_profile.as_deref(), Some("standard"));
        assert_eq!(
            c.paths.ort_library,
            Some(Path::new("/opt/veriface").join("onnxruntime.dll"))
        );
    }

    #[test]
    fn rejects_unknown_keys_and_missing_api_keys() {
        let typo = MINIMAL.replace("[paths]", "[paths]\nmodel_dir = \"x\"");
        assert!(Config::from_toml(&typo, Path::new(".")).is_err());
        let no_keys = "[paths]\nmodels = \"m\"\nprofiles = \"p\"\n";
        let e = Config::from_toml(no_keys, Path::new(".")).unwrap_err();
        assert!(e.problems[0].contains("api_keys"));
    }

    #[test]
    fn dates_convert_to_unix_seconds() {
        let t = |text: &str| unix_seconds(&text.parse().unwrap());
        assert_eq!(t("1970-01-01"), Ok(0));
        assert_eq!(t("2026-01-01"), Ok(1_767_225_600));
        assert_eq!(t("2026-02-01T12:00:00Z"), Ok(1_769_947_200));
        assert_eq!(
            t("2026-02-01T12:00:00"),
            Ok(1_769_947_200),
            "no offset: UTC"
        );
        assert_eq!(t("2026-02-01T16:00:00+04:00"), Ok(1_769_947_200));
        assert_eq!(t("2000-02-29T00:00:01Z"), Ok(951_782_401));
        assert!(t("12:00:00").is_err());
    }

    #[test]
    fn keys_may_share_a_project_but_not_a_label_or_hash() {
        let key = |project: &str, label: &str, sha: char, extra: &str| {
            format!(
                "[[api_keys]]\nproject = \"{project}\"\nlabel = \"{label}\"\nsha256 = \"{}\"\nprofiles = [\"*\"]\n{extra}",
                sha.to_string().repeat(64)
            )
        };
        let paths = "[paths]\nmodels = \"m\"\nprofiles = \"p\"\n";
        let two = format!(
            "{paths}{}{}",
            key("crm", "a", '1', ""),
            key("crm", "b", '2', "")
        );
        assert_eq!(
            Config::from_toml(&two, Path::new("."))
                .unwrap()
                .api_keys
                .len(),
            2
        );
        let same_label = format!(
            "{paths}{}{}",
            key("crm", "a", '1', ""),
            key("hr", "a", '2', "")
        );
        assert!(Config::from_toml(&same_label, Path::new(".")).is_err());

        let bad = format!(
            "{paths}{}",
            key(
                "x",
                "x",
                '3',
                "burst = 5\nmax_concurrency = 0\nnot_before = 2027-01-01\nexpires = 2026-01-01\n"
            )
        );
        let e = Config::from_toml(&bad, Path::new(".")).unwrap_err();
        assert_eq!(e.problems.len(), 3, "{e}");
        let ok = format!(
            "{paths}{}",
            key(
                "x",
                "x",
                '3',
                "rate_per_minute = 600\nburst = 20\nmax_concurrency = 4\nexpires = 2027-01-01T00:00:00Z\n"
            )
        );
        let c = Config::from_toml(&ok, Path::new(".")).unwrap();
        assert_eq!(c.api_keys[0].rate_per_minute, Some(600));
        assert_eq!(c.reload.watch_interval_s, 10);
        assert_eq!(c.logging.max_files, 10);
    }
}
