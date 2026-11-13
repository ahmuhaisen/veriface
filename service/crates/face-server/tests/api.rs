//! Router-level tests with the stub engine (`tower::ServiceExt::oneshot`): auth, every problem
//! type, multipart validation, limits, overload, timeouts, panics, health, profiles, metrics and
//! request ids. Every JSON response is validated against `contracts/openapi.yaml` (field names,
//! required fields, types and enum values; unknown fields are rejected).

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use face_imaging::RgbImage;
use face_imaging::codec::encode_jpeg;
use face_server::api::Settings;
use face_server::auth::sha256_hex;
use face_server::engine::StubEngine;

use common::*;

/// Multipart parts: `(name, bytes)`.
type Parts<'a> = &'a [(&'a str, &'a [u8])];
/// A named mutation of a JSON document.
type Mutation = (&'static str, fn(&mut serde_json::Value));

// ------------------------------------------------------------------- successful results

#[tokio::test]
async fn quality_result_matches_the_contract() {
    let (_, router) = stub_app();
    let reply = send(
        &router,
        post("/v1/quality", Some(KEY_ALL), &[("image", &jpeg(64, 48))]),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let json = conforms(&reply, "/v1/quality", "post");

    assert_eq!(json["requestId"], reply.request_id());
    assert_eq!(json["subject"], "selfie");
    assert_eq!(json["verdict"], "undetermined");
    assert_eq!(json["profile"]["name"], "standard");
    assert_eq!(json["profile"]["sha256"], sha256_hex(STANDARD));
    assert!(
        json["engine"]["version"]
            .as_str()
            .unwrap()
            .ends_with("+stub")
    );
    assert_eq!(json["engine"]["catalogueVersion"], 1);
    assert!(json["face"].is_null() && json["output"].is_null());
    for stage in ["totalMs", "queueMs", "uploadMs", "decodeMs", "evaluateMs"] {
        assert!(
            json["timings"][stage].is_number(),
            "missing timings.{stage}"
        );
    }
    let checks = json["checks"].as_array().unwrap();
    assert!(
        checks
            .iter()
            .any(|c| c["id"] == "FACE_PRESENT" && c["status"] == "undetermined")
    );
    assert!(
        checks
            .iter()
            .any(|c| c["id"] == "GAZE_FRONTAL" && c["status"] == "skipped")
    );
    assert!(checks.iter().all(|c| c["value"].is_null()));
    assert_eq!(json["reasons"], serde_json::json!([]));
}

#[tokio::test]
async fn the_contract_checker_rejects_wrong_shapes() {
    // The shape assertions above are only as good as this checker.
    let (_, router) = stub_app();
    let reply = send(
        &router,
        post("/v1/quality", Some(KEY_ALL), &[("image", &jpeg(16, 16))]),
    )
    .await;
    let good = reply.json();
    let c = contract();
    let schema = c.schema("QualityResult");
    assert!(c.validate(&good, schema).is_empty());

    let mutations: [Mutation; 7] = [
        ("renamed field", |v| {
            let id = v["requestId"].take();
            v.as_object_mut().unwrap().remove("requestId");
            v["requestID"] = id;
        }),
        ("unknown enum value", |v| v["verdict"] = "maybe".into()),
        ("wrong const", |v| v["subject"] = "document".into()),
        ("missing required nested field", |v| {
            v["checks"][0].as_object_mut().unwrap().remove("unit");
        }),
        ("unknown check status", |v| {
            v["checks"][0]["status"] = "unknown".into()
        }),
        ("wrong type", |v| v["timings"]["decodeMs"] = "fast".into()),
        ("unexpected nested field", |v| {
            v["engine"]["extra"] = 1.into()
        }),
    ];
    for (what, mutate) in mutations {
        let mut bad = good.clone();
        mutate(&mut bad);
        assert!(!c.validate(&bad, schema).is_empty(), "not caught: {what}");
    }
}

#[tokio::test]
async fn document_portrait_result_matches_the_contract() {
    let (_, router) = stub_app();
    let reply = send(
        &router,
        post(
            "/v1/document-portraits?profile=icao-strict&output=false",
            Some(KEY_ALL),
            &[("document", &png(80, 50))],
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
    assert_eq!(json["subject"], "document");
    assert_eq!(json["profile"]["name"], "icao-strict");
    assert_eq!(json["document"]["detected"], false);
    assert_eq!(json["document"]["orientationDegrees"], 0);
    assert!(json["portraitSourceBox"].is_null());
    let ids: Vec<&str> = json["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"PORTRAIT_FOUND") && ids.contains(&"DOCUMENT_DETECTED"));
    assert!(
        !ids.contains(&"FACE_PRESENT"),
        "selfie-only checks are not evaluated"
    );
}

#[tokio::test]
async fn verification_result_matches_the_contract_and_is_review_without_a_score() {
    let (_, router) = stub_app();
    let parts: &[(&str, &[u8])] = &[("probe", &jpeg(64, 64)), ("reference", &png(64, 40))];

    let reply = send(&router, post("/v1/verifications", Some(KEY_ALL), parts)).await;
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    let json = conforms(&reply, "/v1/verifications", "post");
    assert_eq!(json["outcome"], "review");
    assert_eq!(json["decision"], "undetermined");
    assert!(json["score"].is_null());
    assert_eq!(json["threshold"], 0.45);
    assert_eq!(json["operatingPoint"]["id"], "fmr-1e-4");
    assert_eq!(json["calibration"]["status"], "provisional");
    assert_eq!(json["reference"]["kind"], "portrait");
    assert!(json["reference"]["document"].is_null());
    assert_eq!(json["risk"]["flagged"], false);
    let reasons = json["reasons"].as_array().unwrap();
    assert!(reasons.contains(&serde_json::json!({"source": "decision", "code": "NO_SCORE"})));
    assert!(reasons.contains(&serde_json::json!({"source": "probe", "code": "FACE_PRESENT"})));

    let reply = send(
        &router,
        post(
            "/v1/verifications?referenceKind=document&operatingPoint=fmr-1e-3",
            Some(KEY_ALL),
            parts,
        ),
    )
    .await;
    let json = conforms(&reply, "/v1/verifications", "post");
    assert_eq!(json["reference"]["kind"], "document");
    assert_eq!(json["reference"]["document"]["detected"], false);
    assert_eq!(json["threshold"], 0.38);
    assert_eq!(json["operatingPoint"]["targetFmr"], 0.001);
}

// --------------------------------------------------------------------------------- auth

#[tokio::test]
async fn api_keys_are_required_and_checked() {
    let (_, router) = stub_app();
    let image = jpeg(32, 32);
    let parts: &[(&str, &[u8])] = &[("image", &image)];

    let reply = send(&router, post("/v1/quality", None, parts)).await;
    assert_problem(&reply, "/v1/quality", "post", 401, "unauthorized");
    let reply = send(&router, post("/v1/quality", Some("wrong-key"), parts)).await;
    assert_problem(&reply, "/v1/quality", "post", 401, "unauthorized");

    let mut twice = post("/v1/quality", Some(KEY_ALL), parts);
    twice
        .headers_mut()
        .append("x-api-key", KEY_STANDARD.parse().unwrap());
    let reply = send(&router, twice).await;
    assert_problem(&reply, "/v1/quality", "post", 401, "unauthorized");

    for path in ["/v1/profiles", "/v1/info"] {
        let reply = send(&router, get(path, None)).await;
        assert_problem(&reply, path, "get", 401, "unauthorized");
    }
}

#[tokio::test]
async fn profiles_are_resolved_and_enforced_per_key() {
    let (_, router) = stub_app();
    let image = jpeg(32, 32);
    let parts: &[(&str, &[u8])] = &[("image", &image)];

    let reply = send(
        &router,
        post("/v1/quality?profile=icao-strict", Some(KEY_STANDARD), parts),
    )
    .await;
    assert_problem(&reply, "/v1/quality", "post", 403, "profile-forbidden");
    let reply = send(
        &router,
        post("/v1/quality?profile=nope", Some(KEY_ALL), parts),
    )
    .await;
    assert_problem(&reply, "/v1/quality", "post", 404, "profile-not-found");

    // No profile in the request: the key's default, else `standard`.
    let reply = send(&router, post("/v1/quality", Some(KEY_TINY), parts)).await;
    assert_eq!(
        conforms(&reply, "/v1/quality", "post")["profile"]["name"],
        "tiny"
    );
    let reply = send(&router, post("/v1/quality", Some(KEY_STANDARD), parts)).await;
    assert_eq!(
        conforms(&reply, "/v1/quality", "post")["profile"]["name"],
        "standard"
    );
}

// ------------------------------------------------------------- request validation (400/415)

#[tokio::test]
async fn query_strings_are_validated() {
    let (_, router) = stub_app();
    let image = jpeg(32, 32);
    let parts: &[(&str, &[u8])] = &[("image", &image)];
    for uri in [
        "/v1/quality?profle=standard",
        "/v1/quality?output=yes",
        "/v1/quality?profile=standard&profile=icao-strict",
    ] {
        let reply = send(&router, post(uri, Some(KEY_ALL), parts)).await;
        assert_problem(&reply, "/v1/quality", "post", 400, "invalid-request");
    }
    let pair: &[(&str, &[u8])] = &[("probe", &image), ("reference", &image)];
    for uri in [
        "/v1/verifications?referenceKind=passport",
        "/v1/verifications?operatingPoint=fmr-1e-9",
    ] {
        let reply = send(&router, post(uri, Some(KEY_ALL), pair)).await;
        assert_problem(&reply, "/v1/verifications", "post", 400, "invalid-request");
    }
    // output=true needs complete output settings in the profile.
    let reply = send(
        &router,
        post(
            "/v1/quality?profile=no-output&output=true",
            Some(KEY_TINY),
            parts,
        ),
    )
    .await;
    assert_problem(&reply, "/v1/quality", "post", 400, "invalid-request");
    let reply = send(
        &router,
        post("/v1/quality?profile=no-output", Some(KEY_TINY), parts),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn multipart_bodies_are_validated() {
    let (_, router) = stub_app();
    let image = jpeg(32, 32);

    // Not multipart at all.
    let request = Request::post("/v1/quality")
        .header("x-api-key", KEY_ALL)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .body(Body::from(image.clone()))
        .unwrap();
    let reply = send(&router, request).await;
    assert_problem(&reply, "/v1/quality", "post", 415, "unsupported-media-type");

    // Multipart without a boundary.
    let request = Request::post("/v1/quality")
        .header("x-api-key", KEY_ALL)
        .header(header::CONTENT_TYPE, "multipart/form-data")
        .body(Body::from(multipart_body(&[("image", &image)])))
        .unwrap();
    let reply = send(&router, request).await;
    assert_problem(&reply, "/v1/quality", "post", 400, "invalid-request");

    let cases: &[(&str, Parts<'_>, &str)] = &[
        ("/v1/quality", &[], "missing part `image`"),
        (
            "/v1/quality",
            &[("photo", &image)],
            "unexpected part `photo`",
        ),
        (
            "/v1/quality",
            &[("image", &image), ("extra", b"x")],
            "unexpected part `extra`",
        ),
        (
            "/v1/quality",
            &[("image", &image), ("image", &image)],
            "more than once",
        ),
        ("/v1/quality", &[("image", b"")], "is empty"),
        (
            "/v1/document-portraits",
            &[("image", &image)],
            "unexpected part `image`",
        ),
        (
            "/v1/verifications",
            &[("probe", &image)],
            "missing part `reference`",
        ),
    ];
    for (path, parts, detail) in cases {
        let reply = send(&router, post(path, Some(KEY_ALL), parts)).await;
        let json = assert_problem(&reply, path, "post", 400, "invalid-request");
        assert!(
            json["detail"].as_str().unwrap().contains(detail),
            "{path}: {json:#}"
        );
    }

    // A truncated body (no closing boundary).
    let mut body = multipart_body(&[("image", &image)]);
    body.truncate(body.len() - 10);
    let request = Request::post("/v1/quality")
        .header("x-api-key", KEY_ALL)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    let reply = send(&router, request).await;
    assert_problem(&reply, "/v1/quality", "post", 400, "invalid-request");
}

// ---------------------------------------------------------------------- limits (413/415/422)

#[tokio::test]
async fn image_limits_map_to_their_problem_types() {
    let (_, router) = stub_app();
    let tiny = |parts: &[(&str, &[u8])]| post("/v1/quality", Some(KEY_TINY), parts);

    // Part over input.maxBytes (4096): rejected while streaming, before decoding.
    let reply = send(&router, tiny(&[("image", &vec![0u8; 5000])])).await;
    assert_problem(&reply, "/v1/quality", "post", 413, "payload-too-large");

    // Declared body far over the limit: rejected before reading.
    let mut request = tiny(&[("image", &vec![0u8; 200_000])]);
    let length = 200_000 + 300;
    request
        .headers_mut()
        .insert(header::CONTENT_LENGTH, length.to_string().parse().unwrap());
    let reply = send(&router, request).await;
    assert_problem(&reply, "/v1/quality", "post", 413, "payload-too-large");

    // Format not allowed by the profile, and not an image at all.
    let small_png = png(16, 16);
    let reply = send(&router, tiny(&[("image", &small_png)])).await;
    let json = assert_problem(&reply, "/v1/quality", "post", 415, "unsupported-media-type");
    assert!(json["detail"].as_str().unwrap().contains("png"));
    let reply = send(
        &router,
        tiny(&[("image", b"GIF89a\x01\x00\x01\x00garbage")]),
    )
    .await;
    assert_problem(&reply, "/v1/quality", "post", 415, "unsupported-media-type");

    // 200x200 = 40 000 pixels > maxPixels (10 000), under 4 KiB.
    let flat = encode_jpeg(&RgbImage::new(200, 200), 30).unwrap();
    assert!(flat.len() < 4096);
    let reply = send(&router, tiny(&[("image", &flat)])).await;
    assert_problem(&reply, "/v1/quality", "post", 422, "image-too-large");

    // JPEG signature followed by garbage.
    let mut corrupt = vec![0xFF, 0xD8, 0xFF, 0xE0];
    corrupt.extend_from_slice(&[0x5A; 100]);
    let reply = send(&router, tiny(&[("image", &corrupt)])).await;
    let json = assert_problem(&reply, "/v1/quality", "post", 422, "image-decode-failed");
    assert!(json["detail"].as_str().unwrap().starts_with("part `image`"));

    // The reference part of a verification is decoded with the same limits.
    let reply = send(
        &router,
        post(
            "/v1/verifications",
            Some(KEY_TINY),
            &[("probe", &jpeg(32, 32)), ("reference", &corrupt)],
        ),
    )
    .await;
    let json = assert_problem(
        &reply,
        "/v1/verifications",
        "post",
        422,
        "image-decode-failed",
    );
    assert!(
        json["detail"]
            .as_str()
            .unwrap()
            .starts_with("part `reference`")
    );
}

// ------------------------------------------------------------------ overload, timeouts, panics

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_beyond_the_queue_are_refused_with_retry_after() {
    let (engine, mut entered) = TestEngine::new(Mode::Block);
    let settings = Settings {
        max_concurrency: 1,
        queue_limit: 1,
        ..Settings::default()
    };
    let (app, router) = test_app(engine.clone(), settings);
    let image = jpeg(32, 32);

    let first = tokio::spawn({
        let router = router.clone();
        let request = post("/v1/quality", Some(KEY_ALL), &[("image", &image)]);
        async move { send(&router, request).await }
    });
    entered.recv().await.unwrap(); // holds the only slot
    let second = tokio::spawn({
        let router = router.clone();
        let request = post("/v1/quality", Some(KEY_ALL), &[("image", &image)]);
        async move { send(&router, request).await }
    });
    while app.metrics.queue_depth.get() < 1 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let refused = send(
        &router,
        post("/v1/quality", Some(KEY_ALL), &[("image", &image)]),
    )
    .await;
    assert_problem(&refused, "/v1/quality", "post", 503, "overloaded");
    assert_eq!(refused.header("retry-after"), Some("1"));

    engine.gate.release();
    assert_eq!(first.await.unwrap().status, StatusCode::OK);
    assert_eq!(second.await.unwrap().status, StatusCode::OK);
    assert_eq!(app.metrics.inflight.get(), 0);
    assert_eq!(app.metrics.queue_depth.get(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_requests_time_out_and_their_slot_is_released_when_the_work_ends() {
    let (engine, _entered) = TestEngine::new(Mode::Block);
    let settings = Settings {
        max_concurrency: 1,
        request_timeout: Duration::from_millis(200),
        ..Settings::default()
    };
    let (app, router) = test_app(engine.clone(), settings);
    let image = jpeg(32, 32);

    let reply = send(
        &router,
        post("/v1/quality", Some(KEY_ALL), &[("image", &image)]),
    )
    .await;
    let json = assert_problem(&reply, "/v1/quality", "post", 503, "overloaded");
    assert!(
        json["detail"]
            .as_str()
            .unwrap()
            .contains("did not complete")
    );
    assert_eq!(
        app.metrics.inflight.get(),
        1,
        "the blocking work still holds the slot"
    );

    engine.gate.release();
    while app.metrics.inflight.get() > 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let reply = send(
        &router,
        post("/v1/quality", Some(KEY_ALL), &[("image", &image)]),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn engine_panics_and_failures_are_problems_and_the_service_stays_up() {
    let image = jpeg(32, 32);
    for (mode, status, code) in [
        (Mode::Panic, 500, "internal"),
        (Mode::Fail, 500, "internal"),
        (Mode::Unavailable, 503, "not-ready"),
    ] {
        let (engine, _entered) = TestEngine::new(mode);
        let (app, router) = test_app(engine, Settings::default());
        let reply = send(
            &router,
            post("/v1/quality", Some(KEY_ALL), &[("image", &image)]),
        )
        .await;
        let json = assert_problem(&reply, "/v1/quality", "post", status, code);
        // Internal details stay in the logs.
        assert!(!reply.body.windows(4).any(|w| w == b"test"), "{json:#}");
        assert_eq!(app.metrics.inflight.get(), 0);
        let reply = send(&router, get("/v1/info", Some(KEY_ALL))).await;
        assert_eq!(reply.status, StatusCode::OK, "still serving after {code}");
    }
}

// ------------------------------------------------------------------------- health, readiness

#[tokio::test]
async fn health_and_readiness() {
    let (app, router) = stub_app();
    let reply = send(&router, get("/health/live", None)).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["status"], "live");
    let reply = send(&router, get("/health/ready", None)).await;
    assert_eq!(reply.status, StatusCode::OK);

    app.set_ready(false);
    let reply = send(&router, get("/health/ready", None)).await;
    assert_problem(&reply, "/health/ready", "get", 503, "not-ready");
    assert_eq!(reply.header("retry-after"), Some("5"));
    let reply = send(
        &router,
        post("/v1/quality", Some(KEY_ALL), &[("image", &jpeg(8, 8))]),
    )
    .await;
    assert_problem(&reply, "/v1/quality", "post", 503, "not-ready");
    // Profiles and info do not need the engine.
    assert_eq!(
        send(&router, get("/v1/profiles", Some(KEY_ALL)))
            .await
            .status,
        StatusCode::OK
    );
    let reply = send(&router, get("/health/live", None)).await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn health_can_require_a_key() {
    let settings = Settings {
        health_requires_key: true,
        ..Settings::default()
    };
    let (_, router) = test_app(Arc::new(StubEngine::new()), settings);
    for path in ["/health/live", "/health/ready"] {
        let reply = send(&router, get(path, None)).await;
        assert_problem(&reply, path, "get", 401, "unauthorized");
        let reply = send(&router, get(path, Some(KEY_STANDARD))).await;
        assert_eq!(reply.status, StatusCode::OK);
    }
}

// ----------------------------------------------------------------------- profiles and info

#[tokio::test]
async fn profile_endpoints() {
    let (_, router) = stub_app();
    let reply = send(&router, get("/v1/profiles", Some(KEY_ALL))).await;
    let json = conforms(&reply, "/v1/profiles", "get");
    let names: Vec<&str> = json["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["icao-strict", "no-output", "relaxed", "standard", "tiny"]
    );

    let reply = send(&router, get("/v1/profiles", Some(KEY_STANDARD))).await;
    let json = conforms(&reply, "/v1/profiles", "get");
    assert_eq!(json["profiles"].as_array().unwrap().len(), 1);
    assert_eq!(json["profiles"][0]["sha256"], sha256_hex(STANDARD));
    assert_eq!(json["profiles"][0]["version"], "0.1.0");

    let reply = send(&router, get("/v1/profiles/standard", Some(KEY_STANDARD))).await;
    let json = conforms(&reply, "/v1/profiles/{name}", "get");
    assert_eq!(json["sha256"], sha256_hex(STANDARD));
    assert_eq!(json["profile"]["name"], "standard");
    assert_eq!(
        json["profile"]["selfie"]["checks"]["HEAD_POSE_YAW"]["max"],
        15.0
    );
    assert_eq!(
        json["profile"]["verification"]["defaultOperatingPoint"],
        "fmr-1e-4"
    );

    let reply = send(&router, get("/v1/profiles/icao-strict", Some(KEY_STANDARD))).await;
    assert_problem(
        &reply,
        "/v1/profiles/{name}",
        "get",
        403,
        "profile-forbidden",
    );
    let reply = send(&router, get("/v1/profiles/unknown", Some(KEY_ALL))).await;
    assert_problem(
        &reply,
        "/v1/profiles/{name}",
        "get",
        404,
        "profile-not-found",
    );
}

#[tokio::test]
async fn info_endpoint() {
    let (_, router) = stub_app();
    let reply = send(&router, get("/v1/info", Some(KEY_STANDARD))).await;
    let json = conforms(&reply, "/v1/info", "get");
    assert_eq!(json["catalogueVersion"], 1);
    assert_eq!(json["models"], serde_json::json!([]));
}

// ---------------------------------------------------------------------------- request ids

#[tokio::test]
async fn request_ids_are_echoed_or_generated() {
    let (_, router) = stub_app();
    let mut request = post("/v1/quality", Some(KEY_ALL), &[("image", &jpeg(16, 16))]);
    request
        .headers_mut()
        .insert("x-request-id", "client-id-42".parse().unwrap());
    let reply = send(&router, request).await;
    assert_eq!(reply.request_id(), "client-id-42");
    assert_eq!(reply.json()["requestId"], "client-id-42");

    // Problems carry it as `instance`.
    let mut request = get("/v1/profiles/nope", Some(KEY_ALL));
    request
        .headers_mut()
        .insert("x-request-id", "client-id-43".parse().unwrap());
    let reply = send(&router, request).await;
    assert_eq!(reply.json()["instance"], "client-id-43");

    // Generated when absent: a UUID.
    let reply = send(&router, get("/health/live", None)).await;
    let id = reply.request_id();
    assert_eq!(id.len(), 36);
    assert_eq!(id.matches('-').count(), 4);

    // Contract: an invalid id (too long, not visible ASCII, sent twice) is replaced by a
    // generated one; the request itself succeeds.
    let too_long = "x".repeat(129);
    for values in [
        vec![too_long.as_str()],
        vec!["has space"],
        vec!["one", "two"],
    ] {
        let mut request = get("/v1/info", Some(KEY_ALL));
        for value in &values {
            request
                .headers_mut()
                .append("x-request-id", value.parse().unwrap());
        }
        let reply = send(&router, request).await;
        assert_eq!(reply.status, 200, "{values:?}");
        let id = reply.request_id();
        assert_eq!(id.len(), 36, "{values:?}");
        assert!(!values.contains(&id), "{values:?}");
    }
    // ... and problems carry the generated id as `instance`.
    let mut request = get("/v1/profiles/nope", Some(KEY_ALL));
    request
        .headers_mut()
        .insert("x-request-id", too_long.parse().unwrap());
    let reply = send(&router, request).await;
    let json = assert_problem(
        &reply,
        "/v1/profiles/{name}",
        "get",
        404,
        "profile-not-found",
    );
    assert_eq!(json["instance"], reply.request_id());
    assert_eq!(reply.request_id().len(), 36);
}

// -------------------------------------------------------------------- routing and metrics

#[tokio::test]
async fn unknown_routes_and_methods_are_problems() {
    let (_, router) = stub_app();
    let reply = send(&router, get("/v2/quality", Some(KEY_ALL))).await;
    assert_problem(&reply, "/v2/quality", "get", 404, "not-found");
    let reply = send(&router, get("/v1/quality", Some(KEY_ALL))).await;
    assert_problem(&reply, "/v1/quality", "get", 405, "method-not-allowed");
}

#[tokio::test]
async fn metrics_expose_requests_checks_and_outcomes() {
    let (_, router) = stub_app();
    let image = jpeg(16, 16);
    send(
        &router,
        post("/v1/quality", Some(KEY_ALL), &[("image", &image)]),
    )
    .await;
    send(
        &router,
        post(
            "/v1/verifications",
            Some(KEY_ALL),
            &[("probe", &image), ("reference", &image)],
        ),
    )
    .await;
    send(&router, get("/v1/info", None)).await;

    let reply = send(&router, get("/metrics", None)).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(
        reply
            .header("content-type")
            .unwrap()
            .starts_with("text/plain")
    );
    let text = String::from_utf8(reply.body).unwrap();
    for expected in [
        "face_requests_total{endpoint=\"/v1/quality\",status=\"200\"} 1",
        "face_requests_total{endpoint=\"/v1/info\",status=\"401\"} 1",
        "face_check_status_total{check=\"FACE_PRESENT\",status=\"undetermined\"} 2",
        "face_check_status_total{check=\"PROBE_REFERENCE_DUPLICATE\",status=\"undetermined\"} 1",
        "face_verification_outcome_total{outcome=\"review\"} 1",
        "face_stage_duration_seconds_count{stage=\"decode\"} 2",
        "face_request_duration_seconds_count{endpoint=\"/v1/verifications\"} 1",
        "face_inflight 0",
        "face_queue_depth 0",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }

    let settings = Settings {
        metrics_enabled: false,
        ..Settings::default()
    };
    let (_, router) = test_app(Arc::new(StubEngine::new()), settings);
    let reply = send(&router, get("/metrics", None)).await;
    assert_problem(&reply, "/metrics", "get", 404, "not-found");
}
