//! `X-API-Key` authentication and the per-key limits. Keys are stored as SHA-256 hashes; the
//! presented key is hashed and compared with every configured hash in constant time.
//!
//! A key may carry a validity window (`not_before`, `expires`: 401 outside it), a request rate
//! (token bucket: `rate_per_minute`, `burst`) and a concurrency limit (`max_concurrency`); both
//! limits answer `429 rate-limited` with `Retry-After`. The limit state of a key (its bucket and
//! in-flight count) lives in a [`KeyState`] shared across hot reloads, matched by the key hash.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use subtle::{ConditionallySelectable, ConstantTimeEq};

use crate::api::{App, RequestLog};
use crate::config::{ApiKeyConfig, unix_seconds};
use crate::problem::{Problem, ProblemKind};
use crate::profiles::ProfileStore;

pub const API_KEY_HEADER: &str = "x-api-key";
/// Profile used when neither the request nor the key names one (contract: "else `standard`").
pub const FALLBACK_PROFILE: &str = "standard";
/// `profiles = ["*"]` allows every loaded profile.
pub const ALL_PROFILES: &str = "*";
/// Keys expiring within this many days are reported at startup and on every reload.
const EXPIRY_WARNING_DAYS: i64 = 14;

/// An authenticated caller.
#[derive(Debug)]
pub struct ApiKey {
    pub project: String,
    /// The key's `label`, for logs.
    pub label: Option<String>,
    digest: [u8; 32],
    access: ProfileAccess,
    default_profile: Option<String>,
    /// Unix seconds.
    not_before: Option<i64>,
    expires: Option<i64>,
    limits: KeyLimits,
    state: Arc<KeyState>,
}

#[derive(Debug)]
enum ProfileAccess {
    All,
    Only(BTreeSet<String>),
}

/// A key's configured limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyLimits {
    pub rate_per_minute: Option<u32>,
    /// Bucket capacity.
    pub burst: u32,
    pub max_concurrency: Option<u32>,
}

impl KeyLimits {
    fn from_config(entry: &ApiKeyConfig) -> Self {
        let burst = entry
            .burst
            .or(entry.rate_per_minute.map(|r| (r / 10).max(1)))
            .unwrap_or(1);
        KeyLimits {
            rate_per_minute: entry.rate_per_minute,
            burst,
            max_concurrency: entry.max_concurrency,
        }
    }
}

/// The runtime state of a key's limits.
#[derive(Debug, Default)]
pub struct KeyState {
    bucket: Mutex<Option<Bucket>>,
    inflight: AtomicU32,
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// Why a key may not be used right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotYetValid,
    Expired,
    /// `429`: the key's request rate; retry after this many seconds.
    Rate(u32),
    /// `429`: the key's concurrency limit.
    Concurrency,
}

/// Holds one of the key's in-flight places until dropped.
#[derive(Debug)]
pub struct InflightGuard(Option<Arc<KeyState>>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        if let Some(state) = &self.0 {
            state.inflight.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl ApiKey {
    pub fn allows(&self, profile: &str) -> bool {
        match &self.access {
            ProfileAccess::All => true,
            ProfileAccess::Only(names) => names.contains(profile),
        }
    }

    /// The key's default profile, else `standard`.
    pub fn default_profile(&self) -> &str {
        self.default_profile.as_deref().unwrap_or(FALLBACK_PROFILE)
    }

    pub fn limits(&self) -> KeyLimits {
        self.limits
    }

    /// Validity window, then concurrency, then rate. On success the guard holds an in-flight
    /// place (when the key has a concurrency limit) until the request ends.
    pub fn admit(&self, now_unix: i64, now: Instant) -> Result<InflightGuard, Refusal> {
        if self.not_before.is_some_and(|t| now_unix < t) {
            return Err(Refusal::NotYetValid);
        }
        if self.expires.is_some_and(|t| now_unix >= t) {
            return Err(Refusal::Expired);
        }
        let guard = match self.limits.max_concurrency {
            Some(max) => {
                let before = self.state.inflight.fetch_add(1, Ordering::AcqRel);
                let guard = InflightGuard(Some(self.state.clone()));
                if before >= max {
                    return Err(Refusal::Concurrency);
                }
                guard
            }
            None => InflightGuard(None),
        };
        if let Some(per_minute) = self.limits.rate_per_minute {
            let rate = f64::from(per_minute) / 60.0;
            let capacity = f64::from(self.limits.burst);
            let mut bucket = self
                .state
                .bucket
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let b = bucket.get_or_insert(Bucket {
                tokens: capacity,
                at: now,
            });
            let elapsed = now.saturating_duration_since(b.at).as_secs_f64();
            b.tokens = (b.tokens + elapsed * rate).min(capacity);
            b.at = now;
            if b.tokens < 1.0 {
                let wait = ((1.0 - b.tokens) / rate).ceil().clamp(1.0, 86_400.0);
                return Err(Refusal::Rate(wait as u32));
            }
            b.tokens -= 1.0;
        }
        Ok(guard)
    }
}

/// The configured keys.
#[derive(Debug, Default)]
pub struct ApiKeys {
    keys: Vec<Arc<ApiKey>>,
}

impl ApiKeys {
    /// Build from configuration, checking every profile reference against the loaded profiles.
    pub fn from_config(
        entries: &[ApiKeyConfig],
        profiles: &ProfileStore,
    ) -> Result<Self, Vec<String>> {
        ApiKeys::from_config_with_state(entries, profiles, &ApiKeys::default())
    }

    /// [`ApiKeys::from_config`], keeping the limit state of keys that `previous` already had
    /// (same hash), so a reload neither refills buckets nor forgets requests in flight.
    pub fn from_config_with_state(
        entries: &[ApiKeyConfig],
        profiles: &ProfileStore,
        previous: &ApiKeys,
    ) -> Result<Self, Vec<String>> {
        let states: HashMap<[u8; 32], Arc<KeyState>> = previous
            .keys
            .iter()
            .map(|k| (k.digest, k.state.clone()))
            .collect();
        let mut problems = Vec::new();
        let mut keys = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            let at = format!("api_keys[{i}] ({})", entry.project);
            let Some(digest) = parse_sha256_hex(&entry.sha256) else {
                problems.push(format!("{at}: sha256 is not 64 hex characters"));
                continue;
            };
            let access = if entry.profiles.iter().any(|p| p == ALL_PROFILES) {
                ProfileAccess::All
            } else {
                for name in &entry.profiles {
                    if profiles.get(name).is_none() {
                        problems.push(format!("{at}: profile `{name}` is not loaded"));
                    }
                }
                ProfileAccess::Only(entry.profiles.iter().cloned().collect())
            };
            let mut window = |value: &Option<toml::value::Datetime>| {
                value.as_ref().and_then(|v| match unix_seconds(v) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        problems.push(format!("{at}: {e}"));
                        None
                    }
                })
            };
            let not_before = window(&entry.not_before);
            let expires = window(&entry.expires);
            let key = ApiKey {
                project: entry.project.clone(),
                label: entry.label.clone(),
                digest,
                access,
                default_profile: entry.default_profile.clone(),
                not_before,
                expires,
                limits: KeyLimits::from_config(entry),
                state: states.get(&digest).cloned().unwrap_or_default(),
            };
            if let Some(default) = &entry.default_profile {
                if profiles.get(default).is_none() {
                    problems.push(format!("{at}: default_profile `{default}` is not loaded"));
                } else if !key.allows(default) {
                    problems.push(format!(
                        "{at}: default_profile `{default}` is not in this key's profiles"
                    ));
                }
            }
            keys.push(Arc::new(key));
        }
        if problems.is_empty() {
            Ok(ApiKeys { keys })
        } else {
            Err(problems)
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Keys that have expired, expire within two weeks, or are not valid yet (for the log).
    pub fn validity_notes(&self, now_unix: i64) -> Vec<String> {
        let name = |k: &ApiKey| match &k.label {
            Some(label) => format!("{} ({label})", k.project),
            None => k.project.clone(),
        };
        let mut notes = Vec::new();
        for k in &self.keys {
            if let Some(t) = k.expires {
                let days = (t - now_unix).div_euclid(86_400);
                if now_unix >= t {
                    notes.push(format!("API key {} has expired", name(k)));
                } else if days < EXPIRY_WARNING_DAYS {
                    notes.push(format!("API key {} expires in {days} days", name(k)));
                }
            }
            if k.not_before.is_some_and(|t| now_unix < t) {
                notes.push(format!("API key {} is not valid yet", name(k)));
            }
        }
        notes
    }

    /// The key whose hash matches `presented`. Every configured hash is compared, in constant
    /// time, whether or not an earlier one matched.
    pub fn authenticate(&self, presented: &[u8]) -> Option<Arc<ApiKey>> {
        let digest: [u8; 32] = Sha256::digest(presented).into();
        let mut found = u32::MAX;
        for (i, key) in self.keys.iter().enumerate() {
            let index = u32::try_from(i).unwrap_or(u32::MAX);
            found.conditional_assign(&index, key.digest.ct_eq(&digest));
        }
        self.keys.get(usize::try_from(found).ok()?).cloned()
    }
}

/// Seconds since the Unix epoch, now.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0F)]));
    }
    out
}

/// Parse 64 hex characters (either case) into a digest.
pub fn parse_sha256_hex(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// Middleware: require exactly one valid `X-API-Key`, apply the key's validity window and
/// limits, and attach the [`ApiKey`] to the request.
pub async fn require_api_key(
    State(app): State<Arc<App>>,
    mut request: Request,
    next: Next,
) -> Response {
    let mut values = request.headers().get_all(API_KEY_HEADER).iter();
    let presented = match (values.next(), values.next()) {
        (Some(value), None) => value.as_bytes(),
        (None, _) => {
            return Problem::with_detail(ProblemKind::Unauthorized, "send an X-API-Key header")
                .into_response();
        }
        (Some(_), Some(_)) => {
            return Problem::with_detail(
                ProblemKind::Unauthorized,
                "send exactly one X-API-Key header",
            )
            .into_response();
        }
    };
    let Some(key) = app.catalog().keys.authenticate(presented) else {
        return Problem::with_detail(ProblemKind::Unauthorized, "unknown API key").into_response();
    };
    if let Some(log) = request.extensions().get::<RequestLog>() {
        log.update(|f| {
            f.project = Some(key.project.clone());
            f.key = key.label.clone();
        });
    }
    // Held until the response is ready: the key's in-flight place, when it has a limit.
    let _inflight = match key.admit(now_unix(), Instant::now()) {
        Ok(guard) => guard,
        Err(refusal) => {
            let (problem, reason) = match refusal {
                Refusal::NotYetValid => (
                    Problem::with_detail(
                        ProblemKind::Unauthorized,
                        "this API key is not valid yet",
                    ),
                    None,
                ),
                Refusal::Expired => (
                    Problem::with_detail(ProblemKind::Unauthorized, "this API key has expired"),
                    None,
                ),
                Refusal::Rate(seconds) => (
                    Problem::with_detail(
                        ProblemKind::RateLimited,
                        "this API key has used up its request rate",
                    )
                    .retry_after(seconds),
                    Some("rate"),
                ),
                Refusal::Concurrency => (
                    Problem::with_detail(
                        ProblemKind::RateLimited,
                        "this API key already has its maximum number of requests in flight",
                    ),
                    Some("concurrency"),
                ),
            };
            if let Some(reason) = reason {
                app.metrics
                    .rate_limited
                    .with_label_values(&[key.project.as_str(), reason])
                    .inc();
            }
            return problem.into_response();
        }
    };
    request.extensions_mut().insert(key);
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(entries: &[(&str, &str, &[&str], Option<&str>)]) -> Result<ApiKeys, Vec<String>> {
        let store = ProfileStore::for_tests();
        let entries: Vec<ApiKeyConfig> = entries
            .iter()
            .map(|(project, key, profiles, default)| ApiKeyConfig {
                project: (*project).to_owned(),
                sha256: sha256_hex(key.as_bytes()),
                profiles: profiles.iter().map(|p| (*p).to_owned()).collect(),
                default_profile: default.map(str::to_owned),
                ..ApiKeyConfig::default()
            })
            .collect();
        ApiKeys::from_config(&entries, &store)
    }

    #[test]
    fn hashes_round_trip() {
        let h = sha256_hex(b"hello");
        assert_eq!(
            h,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(
            parse_sha256_hex(&h.to_uppercase()).map(|d| hex(&d)),
            Some(h)
        );
        assert_eq!(parse_sha256_hex("zz"), None);
        assert_eq!(parse_sha256_hex(&"é".repeat(32)), None);
    }

    #[test]
    fn authenticates_by_hash_and_enforces_profiles() {
        let keys = keys(&[
            ("a", "key-a", &["standard"], None),
            ("b", "key-b", &["*"], Some("icao-strict")),
        ])
        .unwrap();
        let a = keys.authenticate(b"key-a").unwrap();
        assert_eq!(a.project, "a");
        assert!(a.allows("standard") && !a.allows("icao-strict"));
        assert_eq!(a.default_profile(), "standard");
        let b = keys.authenticate(b"key-b").unwrap();
        assert!(b.allows("icao-strict") && b.allows("anything"));
        assert_eq!(b.default_profile(), "icao-strict");
        assert!(keys.authenticate(b"key-c").is_none());
        assert!(keys.authenticate(b"").is_none());
    }

    #[test]
    fn rejects_unknown_profiles_and_bad_defaults() {
        let problems = keys(&[
            ("a", "k1", &["nope"], None),
            ("b", "k2", &["standard"], Some("icao-strict")),
            ("c", "k3", &["*"], Some("missing")),
        ])
        .unwrap_err();
        assert_eq!(problems.len(), 3, "{problems:#?}");
    }

    fn limited(key: &str, edit: impl FnOnce(&mut ApiKeyConfig)) -> ApiKeyConfig {
        let mut entry = ApiKeyConfig {
            project: "p".to_owned(),
            sha256: sha256_hex(key.as_bytes()),
            profiles: vec!["*".to_owned()],
            ..ApiKeyConfig::default()
        };
        edit(&mut entry);
        entry
    }

    fn datetime(text: &str) -> toml::value::Datetime {
        text.parse().unwrap()
    }

    #[test]
    fn validity_window_is_enforced() {
        let store = ProfileStore::for_tests();
        let entry = limited("k", |e| {
            e.not_before = Some(datetime("2026-01-01"));
            e.expires = Some(datetime("2026-02-01T12:00:00Z"));
        });
        let keys = ApiKeys::from_config(&[entry], &store).unwrap();
        let key = keys.authenticate(b"k").unwrap();
        let jan1 = 1_767_225_600; // 2026-01-01T00:00:00Z
        let feb1_noon = 1_769_947_200; // 2026-02-01T12:00:00Z
        let now = Instant::now();
        assert_eq!(key.admit(jan1 - 1, now).unwrap_err(), Refusal::NotYetValid);
        assert!(key.admit(jan1, now).is_ok());
        assert!(key.admit(feb1_noon - 1, now).is_ok());
        assert_eq!(key.admit(feb1_noon, now).unwrap_err(), Refusal::Expired);
        // Notes: expired, expiring soon, not valid yet.
        assert_eq!(keys.validity_notes(feb1_noon), ["API key p has expired"]);
        assert_eq!(
            keys.validity_notes(feb1_noon - 3 * 86_400),
            ["API key p expires in 3 days"]
        );
        assert_eq!(
            keys.validity_notes(jan1 - 10),
            ["API key p is not valid yet"]
        );
    }

    #[test]
    fn concurrency_limit_counts_requests_in_flight() {
        let store = ProfileStore::for_tests();
        let keys =
            ApiKeys::from_config(&[limited("k", |e| e.max_concurrency = Some(2))], &store).unwrap();
        let key = keys.authenticate(b"k").unwrap();
        let now = Instant::now();
        let a = key.admit(0, now).unwrap();
        let b = key.admit(0, now).unwrap();
        assert_eq!(key.admit(0, now).unwrap_err(), Refusal::Concurrency);
        drop(a);
        let c = key.admit(0, now).unwrap();
        drop((b, c));
        assert_eq!(key.state.inflight.load(Ordering::Acquire), 0);
    }

    #[test]
    fn rate_limit_is_a_token_bucket() {
        let store = ProfileStore::for_tests();
        let entry = limited("k", |e| {
            e.rate_per_minute = Some(60); // one per second
            e.burst = Some(3);
        });
        let keys = ApiKeys::from_config(&[entry], &store).unwrap();
        let key = keys.authenticate(b"k").unwrap();
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(key.admit(0, t0).is_ok(), "the burst");
        }
        assert_eq!(key.admit(0, t0).unwrap_err(), Refusal::Rate(1));
        // One token per second refills.
        let t1 = t0 + std::time::Duration::from_millis(1000);
        assert!(key.admit(0, t1).is_ok());
        assert_eq!(key.admit(0, t1).unwrap_err(), Refusal::Rate(1));
        // Slow rates ask for longer waits; the default burst is rate / 10.
        let slow =
            ApiKeys::from_config(&[limited("s", |e| e.rate_per_minute = Some(6))], &store).unwrap();
        let key = slow.authenticate(b"s").unwrap();
        assert_eq!(key.limits().burst, 1);
        assert!(key.admit(0, t0).is_ok());
        assert_eq!(key.admit(0, t0).unwrap_err(), Refusal::Rate(10));
    }

    #[test]
    fn limit_state_survives_a_reload_of_the_same_key() {
        let store = ProfileStore::for_tests();
        let entry = limited("k", |e| {
            e.rate_per_minute = Some(60);
            e.burst = Some(1);
            e.max_concurrency = Some(1);
        });
        let before = ApiKeys::from_config(std::slice::from_ref(&entry), &store).unwrap();
        let now = Instant::now();
        let held = before.authenticate(b"k").unwrap().admit(0, now).unwrap();
        // Reloaded: same hash, so the same bucket (empty) and in-flight count (1).
        let mut relabelled = entry.clone();
        relabelled.label = Some("renamed".to_owned());
        let after = ApiKeys::from_config_with_state(&[relabelled], &store, &before).unwrap();
        let key = after.authenticate(b"k").unwrap();
        assert_eq!(key.label.as_deref(), Some("renamed"));
        assert_eq!(key.admit(0, now).unwrap_err(), Refusal::Concurrency);
        drop(held);
        assert_eq!(key.admit(0, now).unwrap_err(), Refusal::Rate(1));
        // A new key starts fresh.
        let fresh =
            ApiKeys::from_config_with_state(&[limited("other", |_| {})], &store, &before).unwrap();
        assert!(fresh.authenticate(b"other").unwrap().admit(0, now).is_ok());
    }
}
