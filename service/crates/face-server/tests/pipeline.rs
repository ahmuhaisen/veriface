//! Router-level tests with the REAL engine (ONNX Runtime + models) on the public-domain and
//! synthetic fixtures in `testdata/`. Every response is checked against `contracts/openapi.yaml`.
//! Skipped (with a `SKIPPED:` line) when `service/third_party/onnxruntime` or the models are
//! absent.

mod common;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::http::StatusCode;
use face_models::test_support;
use face_models::{Runtime, RuntimeOptions, ids};
use face_server::api::{App, Settings};
use face_server::pipeline::PipelineEngine;
use serde_json::Value;

use common::*;

/// The loaded runtime (1 intra-op thread for every model), shared by every engine in this test
/// binary (loading the models is slow).
fn runtime() -> Option<Arc<Runtime>> {
    static RUNTIME: OnceLock<Option<Arc<Runtime>>> = OnceLock::new();
    RUNTIME.get_or_init(|| load_runtime(&[])).clone()
}

/// A runtime with 2 intra-op threads for AuraFace and the segmenter.
fn runtime_with_overrides() -> Option<Arc<Runtime>> {
    static RUNTIME: OnceLock<Option<Arc<Runtime>>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| load_runtime(&[(ids::EMBEDDER, 2), (ids::SEGMENTER, 2)]))
        .clone()
}

fn load_runtime(overrides: &[(&str, usize)]) -> Option<Arc<Runtime>> {
    let Some(library) = test_support::ort_library() else {
        test_support::skip(
            "ONNX Runtime not found; run service/scripts/fetch-onnxruntime.ps1 (or set ORT_DYLIB_PATH)",
        );
        return None;
    };
    let models = test_support::models_dir();
    if !models.join("manifest.json").is_file() {
        test_support::skip(&format!("no manifest.json in {}", models.display()));
        return None;
    }
    face_models::init_onnxruntime(&library, None).expect("ONNX Runtime loads");
    let mut options = RuntimeOptions {
        sessions_per_model: 2,
        ..RuntimeOptions::default()
    };
    for (id, threads) in overrides {
        options
            .intra_op_threads_per_model
            .insert((*id).to_owned(), *threads);
    }
    let runtime = Runtime::load(&models, &options).expect("models load");
    runtime.warm_up().expect("warm-up");
    Some(Arc::new(runtime))
}

fn engine_app(parallel: bool) -> Option<(Arc<App>, Router)> {
    engine_app_on(runtime()?, parallel)
}

fn engine_app_on(runtime: Arc<Runtime>, parallel: bool) -> Option<(Arc<App>, Router)> {
    let engine = PipelineEngine::new(runtime)
        .expect("engine builds")
        .with_parallel_stages(parallel);
    // Debug builds are slow and the tests run in parallel: no request timeout in practice.
    let settings = Settings {
        request_timeout: std::time::Duration::from_secs(1800),
        max_concurrency: 4,
        queue_limit: 64,
        ..Settings::default()
    };
    Some(test_app(Arc::new(engine), settings))
}

/// The default engine (parallel stages on), built once.
fn app() -> Option<&'static (Arc<App>, Router)> {
    static APP: OnceLock<Option<(Arc<App>, Router)>> = OnceLock::new();
    APP.get_or_init(|| engine_app(true)).as_ref()
}

fn fixture(relative: &str) -> Vec<u8> {
    let path = test_support::testdata_dir().join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", Path::new(&path).display()))
}

fn status(json: &Value, check: &str) -> String {
    json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == check)
        .map(|c| c["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| "absent".to_owned())
}

#[tokio::test]
async fn quality_on_a_portrait() {
    let Some((_, router)) = app() else { return };
    let image = fixture("images/nasa-watkins.jpg");
    let reply = send(
        router,
        post("/v1/quality", Some(KEY_ALL), &[("image", &image)]),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let json = conforms(&reply, "/v1/quality", "post");
    println!(
        "quality nasa-watkins: verdict {} reasons {} warnings {} timings {}",
        json["verdict"], json["reasons"], json["warnings"], json["timings"]
    );
    for check in [
        "FACE_PRESENT",
        "SINGLE_FACE",
        "INTER_EYE_DISTANCE",
        "HEAD_POSE_YAW",
    ] {
        assert_eq!(status(&json, check), "pass", "{check}: {json:#}");
    }
    assert_eq!(status(&json, "COMPRESSION_QUALITY"), "pass");
    let face = &json["face"];
    assert!(
        face["interEyeDistancePx"].as_f64().unwrap() > 60.0,
        "{face:#}"
    );
    assert!(face["pose"]["yaw"].is_number());
    // `standard` produces a portrait-3x4 JPEG within 100 KiB.
    let output = &json["output"];
    assert_eq!(output["mediaType"], "image/jpeg", "{output}");
    assert_eq!(
        (output["width"].as_u64(), output["height"].as_u64()),
        (Some(480), Some(640))
    );
    assert!(output["bytes"].as_u64().unwrap() <= 102_400);
    for stage in [
        "decodeMs",
        "detectMs",
        "landmarksMs",
        "measureMs",
        "evaluateMs",
    ] {
        assert!(json["timings"][stage].is_number(), "timings.{stage}");
    }
}

#[tokio::test]
async fn document_portraits_upright_and_rotated() {
    let Some((_, router)) = app() else { return };
    for (file, degrees) in [("doc-watkins.jpg", 0), ("doc-watkins-rot180.jpg", 180)] {
        let document = fixture(&format!("documents/{file}"));
        let reply = send(
            router,
            post(
                "/v1/document-portraits",
                Some(KEY_ALL),
                &[("document", &document)],
            ),
        )
        .await;
        assert_eq!(
            reply.status,
            StatusCode::OK,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let json = conforms(&reply, "/v1/document-portraits", "post");
        println!(
            "document {file}: verdict {} document {} reasons {} warnings {}",
            json["verdict"], json["document"], json["reasons"], json["warnings"]
        );
        assert_eq!(status(&json, "PORTRAIT_FOUND"), "pass", "{file}: {json:#}");
        assert_eq!(json["document"]["detected"], true, "{file}");
        assert_eq!(json["document"]["orientationDegrees"], degrees, "{file}");
        assert!(
            json["portraitSourceBox"].is_object() && json["face"].is_object(),
            "{file}"
        );
        assert_eq!(json["output"]["mediaType"], "image/jpeg", "{file}");
        // The reference face is reported in input coordinates: inside the source box.
        let (bx, fx) = (
            json["portraitSourceBox"]["x"].as_f64().unwrap(),
            json["face"]["box"]["x"].as_f64().unwrap(),
        );
        let width = json["portraitSourceBox"]["width"].as_f64().unwrap();
        assert!(
            fx > bx - width && fx < bx + 2.0 * width,
            "{file}: face box {fx} vs source {bx}"
        );
    }
}

async fn verification(profile: &str, probe: &str, reference: &str) -> Value {
    let (_, router) = app().unwrap();
    let probe = fixture(probe);
    let reference = fixture(reference);
    let reply = send(
        router,
        post(
            &format!("/v1/verifications?profile={profile}"),
            Some(KEY_ALL),
            &[("probe", &probe), ("reference", &reference)],
        ),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    conforms(&reply, "/v1/verifications", "post")
}

#[tokio::test]
async fn verification_genuine() {
    if app().is_none() {
        return;
    }
    // `standard`: the card portrait (inter-eye distance ~56 px) is below the 60 px minimum.
    let json = verification(
        "standard",
        "images/nasa-watkins.jpg",
        "documents/doc-watkins.jpg",
    )
    .await;
    assert_eq!(json["outcome"], "retake", "{json:#}");
    assert!(json["score"].is_null());
    assert_eq!(
        json["reasons"][0],
        serde_json::json!({"source": "reference", "code": "INTER_EYE_DISTANCE"})
    );

    let json = verification(
        "relaxed",
        "images/nasa-watkins.jpg",
        "documents/doc-watkins.jpg",
    )
    .await;
    println!(
        "genuine: outcome {} decision {} score {} reasons {}",
        json["outcome"], json["decision"], json["score"], json["reasons"]
    );
    assert!(
        json["outcome"] == "verified" || json["outcome"] == "review",
        "{json:#}"
    );
    assert!(json["score"].as_f64().unwrap() > 0.45, "{}", json["score"]);
    assert_eq!(json["reference"]["kind"], "document");
    assert!(json["probe"]["face"].is_object() && json["reference"]["face"].is_object());
    assert!(json["timings"]["embedMs"].is_number());
}

#[tokio::test]
async fn verification_impostor() {
    if app().is_none() {
        return;
    }
    let json = verification(
        "relaxed",
        "images/nasa-glover.jpg",
        "documents/doc-watkins.jpg",
    )
    .await;
    println!(
        "impostor: outcome {} decision {} score {} reasons {}",
        json["outcome"], json["decision"], json["score"], json["reasons"]
    );
    assert_eq!(json["outcome"], "rejected", "{json:#}");
    assert_eq!(json["decision"], "no_match");
    assert!(json["score"].as_f64().unwrap() < json["threshold"].as_f64().unwrap());
}

#[tokio::test]
async fn verification_retake_on_a_low_resolution_document() {
    if app().is_none() {
        return;
    }
    let json = verification(
        "standard",
        "images/nasa-almansoori.jpg",
        "documents/doc-almansoori-lowres.jpg",
    )
    .await;
    println!(
        "retake: outcome {} decision {} score {} reasons {}",
        json["outcome"], json["decision"], json["score"], json["reasons"]
    );
    assert_eq!(json["outcome"], "retake", "{json:#}");
    assert!(json["score"].is_null());
    assert!(
        json["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["source"] == "reference"),
        "{}",
        json["reasons"]
    );
}

#[tokio::test]
async fn info_lists_the_manifest_models() {
    let Some((_, router)) = app() else { return };
    let reply = send(router, get("/v1/info", Some(KEY_ALL))).await;
    let json = conforms(&reply, "/v1/info", "get");
    let ids: Vec<&str> = json["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"auraface-v1") && ids.contains(&"mp-face-landmarker"),
        "{ids:?}"
    );
    assert!(!json["version"].as_str().unwrap().contains("stub"));
}

/// Every check status (probe, reference, risk), outcome and decision, in a comparable form.
fn statuses(json: &Value) -> Vec<String> {
    let mut out = vec![
        format!("outcome={}", json["outcome"]),
        format!("decision={}", json["decision"]),
        format!("kind={}", json["reference"]["kind"]),
    ];
    for (part, checks) in [
        ("probe", &json["probe"]["checks"]),
        ("reference", &json["reference"]["checks"]),
        ("risk", &json["risk"]["checks"]),
    ] {
        for c in checks.as_array().unwrap() {
            out.push(format!("{part}.{}={}", c["id"], c["status"]));
        }
    }
    out
}

/// Run the 12-pair fixture set through two engines: identical check statuses, outcome, decision
/// and reference kind, scores within 1e-4. Returns the largest score difference. All pairs in
/// release builds; the first four in (slow) debug builds.
async fn compare(label: &str, a: &Router, b: &Router) -> f64 {
    let pairs = [
        ("standard", "nasa-watkins", "doc-watkins"),
        ("relaxed", "nasa-watkins", "doc-watkins"),
        ("relaxed", "nasa-watkins", "doc-watkins-rot90"),
        ("relaxed", "nasa-watkins", "doc-watkins-glare"),
        ("relaxed", "nasa-glover", "doc-watkins"),
        ("relaxed", "nasa-almansoori", "doc-almansoori"),
        ("standard", "nasa-almansoori", "doc-almansoori-lowres"),
        ("relaxed", "nasa-menon", "doc-menon"),
        ("relaxed", "nasa-ochoa", "doc-ochoa-scan"),
        ("relaxed", "nasa-chari", "doc-twofaces"),
        ("relaxed", "nasa-meir", "doc-noface"),
        ("relaxed", "nasa-moghbeli", "doc-menon"),
    ];
    let count = if cfg!(debug_assertions) {
        4
    } else {
        pairs.len()
    };
    let mut max_diff: f64 = 0.0;
    for (profile, probe, reference) in pairs.into_iter().take(count) {
        let probe_bytes = fixture(&format!("images/{probe}.jpg"));
        let reference_bytes = fixture(&format!("documents/{reference}.jpg"));
        let mut results = Vec::new();
        for router in [a, b] {
            let reply = send(
                router,
                post(
                    &format!("/v1/verifications?profile={profile}"),
                    Some(KEY_ALL),
                    &[("probe", &probe_bytes), ("reference", &reference_bytes)],
                ),
            )
            .await;
            assert_eq!(reply.status, StatusCode::OK);
            results.push(conforms(&reply, "/v1/verifications", "post"));
        }
        let (x, y) = (&results[0], &results[1]);
        assert_eq!(
            statuses(x),
            statuses(y),
            "{label}: {profile} {probe} vs {reference}"
        );
        let diff = match (x["score"].as_f64(), y["score"].as_f64()) {
            (Some(p), Some(q)) => (p - q).abs(),
            (None, None) => 0.0,
            other => panic!("{label}: {probe} vs {reference}: scores {other:?}"),
        };
        assert!(
            diff <= 1e-4,
            "{label}: {probe} vs {reference}: score difference {diff}"
        );
        max_diff = max_diff.max(diff);
        println!(
            "{label} {profile:8} {probe:16} vs {reference:22}: {} score {} (difference {diff:e})",
            x["outcome"], x["score"]
        );
    }
    println!("{label}: {count} pairs, identical statuses, max score difference {max_diff:e}");
    max_diff
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parallel_stages_change_nothing() {
    let Some((_, parallel)) = engine_app(true) else {
        return;
    };
    let (_, sequential) = engine_app(false).unwrap();
    compare("parallel-vs-sequential", &parallel, &sequential).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_thread_overrides_keep_results() {
    let Some((_, one_thread)) = engine_app(true) else {
        return;
    };
    let (_, two_threads) = engine_app_on(runtime_with_overrides().unwrap(), true).unwrap();
    compare("2-threads-vs-1", &two_threads, &one_thread).await;
}
