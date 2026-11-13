//! Per-key limits through the router (`429 rate-limited`, expired keys) and hot reload of the
//! keys and profiles from files.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use face_server::api::{self, App, Settings};
use face_server::auth::{ApiKeys, sha256_hex};
use face_server::config::{ApiKeyConfig, Config};
use face_server::engine::StubEngine;
use face_server::profiles::ProfileStore;
use face_server::{reload, server};

use common::*;

fn app_with(keys: Vec<ApiKeyConfig>) -> (Arc<App>, axum::Router) {
    let profiles =
        ProfileStore::from_sources([("standard.yaml".to_owned(), STANDARD.to_vec())]).unwrap();
    let keys = ApiKeys::from_config(&keys, &profiles).unwrap();
    let app = App::new(
        Arc::new(StubEngine::new()),
        profiles,
        keys,
        Settings::default(),
    );
    app.set_ready(true);
    let router = api::router(app.clone());
    (app, router)
}

fn entry(key: &str, edit: impl FnOnce(&mut ApiKeyConfig)) -> ApiKeyConfig {
    let mut e = key_entry(key);
    edit(&mut e);
    e
}

fn key_entry(key: &str) -> ApiKeyConfig {
    common::key("crm", key, &["*"], None)
}

#[tokio::test]
async fn rate_limited_keys_get_429_with_retry_after() {
    let (app, router) = app_with(vec![
        entry("limited", |e| {
            e.label = Some("crm-2026".to_owned());
            e.rate_per_minute = Some(60);
            e.burst = Some(2);
        }),
        key_entry("free"),
    ]);
    for _ in 0..2 {
        assert_eq!(
            send(&router, get("/v1/info", Some("limited"))).await.status,
            200
        );
    }
    let reply = send(&router, get("/v1/info", Some("limited"))).await;
    let json = assert_problem(&reply, "/v1/info", "get", 429, "rate-limited");
    assert_eq!(reply.header("retry-after"), Some("1"));
    assert!(json["detail"].as_str().unwrap().contains("rate"));
    // Image endpoints too, before any body is read.
    let reply = send(
        &router,
        post("/v1/quality", Some("limited"), &[("image", &jpeg(16, 16))]),
    )
    .await;
    assert_problem(&reply, "/v1/quality", "post", 429, "rate-limited");
    // Other keys of the same project are not affected.
    assert_eq!(
        send(&router, get("/v1/info", Some("free"))).await.status,
        200
    );
    let metrics = app.metrics.render();
    assert!(
        metrics.contains(r#"face_rate_limited_total{project="crm",reason="rate"} 2"#),
        "{metrics}"
    );
}

#[tokio::test]
async fn expired_and_future_keys_are_unauthorized() {
    let (_, router) = app_with(vec![
        entry("old", |e| e.expires = Some("2000-01-01".parse().unwrap())),
        entry("future", |e| {
            e.not_before = Some("2999-01-01".parse().unwrap())
        }),
    ]);
    let reply = send(&router, get("/v1/info", Some("old"))).await;
    let json = assert_problem(&reply, "/v1/info", "get", 401, "unauthorized");
    assert_eq!(json["detail"], "this API key has expired");
    let reply = send(&router, get("/v1/info", Some("future"))).await;
    let json = assert_problem(&reply, "/v1/info", "get", 401, "unauthorized");
    assert_eq!(json["detail"], "this API key is not valid yet");
}

// ---------------------------------------------------------------------------------- reload

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn keys_toml(entries: &[(&str, &str)]) -> String {
    entries
        .iter()
        .map(|(project, key)| {
            format!(
                "[[api_keys]]\nproject = \"{project}\"\nsha256 = \"{}\"\nprofiles = [\"*\"]\n",
                sha256_hex(key.as_bytes())
            )
        })
        .collect()
}

/// A stub-engine configuration in a temporary directory: config.toml, keys.toml, profiles/.
fn setup(name: &str) -> (Dir, PathBuf) {
    let dir =
        std::env::temp_dir().join(format!("veriface-reload-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("profiles")).unwrap();
    std::fs::write(dir.join("profiles/standard.yaml"), STANDARD).unwrap();
    std::fs::write(dir.join("keys.toml"), keys_toml(&[("crm", "key-1")])).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "api_keys_file = \"keys.toml\"\n[engine]\nkind = \"stub\"\n[paths]\nmodels = \"models\"\nprofiles = \"profiles\"\n",
    )
    .unwrap();
    (Dir(dir), config)
}

fn start(config_path: &Path) -> (Arc<App>, Config) {
    let config = Config::load(config_path).unwrap();
    let engine = server::build_engine(&config).unwrap();
    let app = server::build_app(&config, engine).unwrap();
    (app, config)
}

#[test]
fn reload_swaps_keys_and_profiles_all_or_nothing() {
    let (dir, config_path) = setup("swap");
    let (app, config) = start(&config_path);
    assert!(app.catalog().keys.authenticate(b"key-1").is_some());

    // A new key, a removed key and a new profile.
    std::fs::write(
        dir.0.join("keys.toml"),
        keys_toml(&[("crm", "key-2"), ("hr", "key-3")]),
    )
    .unwrap();
    std::fs::write(
        dir.0.join("profiles/strict.yaml"),
        profile_text(ICAO_STRICT),
    )
    .unwrap();
    let held = app.catalog(); // a request in flight keeps its snapshot
    let reloaded = reload::reload(&app, &config_path, &config, None).unwrap();
    assert_eq!((reloaded.profiles, reloaded.keys), (2, 2));
    assert!(reloaded.needs_restart.is_empty());
    let now = app.catalog();
    assert!(now.keys.authenticate(b"key-1").is_none());
    assert!(now.keys.authenticate(b"key-2").is_some());
    assert!(now.profiles.get("icao-strict").is_some());
    assert!(
        held.keys.authenticate(b"key-1").is_some(),
        "old snapshot unchanged"
    );

    // A broken keys file: refused, the running set stays.
    std::fs::write(dir.0.join("keys.toml"), "[[api_keys]]\nproject = \"x\"\n").unwrap();
    let problems = reload::reload(&app, &config_path, &config, None).unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains("keys.toml")),
        "{problems:?}"
    );
    assert!(app.catalog().keys.authenticate(b"key-2").is_some());

    // A key that refers to a missing profile: refused as a whole.
    std::fs::write(
        dir.0.join("keys.toml"),
        "[[api_keys]]\nproject = \"x\"\nsha256 = \"".to_owned()
            + &sha256_hex(b"key-4")
            + "\"\nprofiles = [\"missing\"]\n",
    )
    .unwrap();
    assert!(reload::reload(&app, &config_path, &config, None).is_err());
    assert!(app.catalog().keys.authenticate(b"key-4").is_none());

    // Profiles that accept larger uploads than the running body limit need a restart.
    std::fs::write(dir.0.join("keys.toml"), keys_toml(&[("crm", "key-2")])).unwrap();
    let bigger = profile_text(STANDARD)
        .replace("name: standard", "name: huge")
        .replace("maxBytes: 10485760", "maxBytes: 52428800");
    std::fs::write(dir.0.join("profiles/huge.yaml"), bigger).unwrap();
    let problems = reload::reload(&app, &config_path, &config, None).unwrap_err();
    assert!(problems[0].contains("restart"), "{problems:?}");
    std::fs::remove_file(dir.0.join("profiles/huge.yaml")).unwrap();

    // Settings outside keys and profiles are reported, not applied.
    let text = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(&config_path, format!("{text}[server]\nqueue_limit = 99\n")).unwrap();
    let reloaded = reload::reload(&app, &config_path, &config, None).unwrap();
    assert_eq!(reloaded.needs_restart, ["server"]);
    assert_eq!(app.settings.queue_limit, 32, "not applied before a restart");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_changes_trigger_a_reload() {
    let (dir, config_path) = setup("watch");
    let text = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        format!("{text}[reload]\nwatch_interval_s = 1\n"),
    )
    .unwrap();
    let fingerprint = reload::Fingerprint::of(&config_path, &Config::load(&config_path).unwrap());
    let (app, config) = start(&config_path);
    let (handle, requests) = reload::channel();
    tokio::spawn(reload::run(
        app.clone(),
        config,
        None,
        fingerprint,
        requests,
    ));

    std::fs::write(dir.0.join("keys.toml"), keys_toml(&[("crm", "key-9")])).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.catalog().keys.authenticate(b"key-9").is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "no reload after a file change"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // An explicit request (SIGHUP / paramchange) reloads too.
    std::fs::write(dir.0.join("keys.toml"), keys_toml(&[("crm", "key-10")])).unwrap();
    handle.request("test");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.catalog().keys.authenticate(b"key-10").is_none() {
        assert!(std::time::Instant::now() < deadline, "no reload on request");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        app.metrics
            .render()
            .contains(r#"face_config_reloads_total{result="ok"}"#)
    );
}
