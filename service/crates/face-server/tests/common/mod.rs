//! Shared helpers for the router-level tests: fixtures, request builders, engine doubles and
//! a checker for the response schemas of `contracts/openapi.yaml`.
// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::io::Write as _;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use face_imaging::RgbImage;
use face_imaging::codec::{encode_jpeg, encode_png};
use face_server::api::{self, App, Settings};
use face_server::auth::{ApiKeys, sha256_hex};
use face_server::config::ApiKeyConfig;
use face_server::dto::{DocumentPortraitBody, EngineInfo, QualityBody, VerificationBody};
use face_server::engine::{
    DocumentPortraitRequest, Engine, EngineError, QualityRequest, Stages, StubEngine,
    VerificationRequest,
};
use face_server::profiles::ProfileStore;
use serde_json::Value;
use tower::ServiceExt;

pub const OPENAPI: &str = include_str!("../../../../../contracts/openapi.yaml");
pub const STANDARD: &[u8] = include_bytes!("../../../../../profiles/standard.yaml");
pub const ICAO_STRICT: &[u8] = include_bytes!("../../../../../profiles/icao-strict.yaml");

/// Allowed every profile; default `standard`.
pub const KEY_ALL: &str = "test-key-all";
/// Allowed `standard` only.
pub const KEY_STANDARD: &str = "test-key-standard";
/// Allowed `tiny` and `no-output`; default `tiny`.
pub const KEY_TINY: &str = "test-key-tiny";

// ------------------------------------------------------------------------------ fixtures

pub fn profile_text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .replace("\r\n", "\n")
}

/// `standard` with 4 KiB / 10 000 pixel limits and JPEG only.
pub fn tiny_profile() -> Vec<u8> {
    let text = profile_text(STANDARD)
        .replace("name: standard", "name: tiny")
        .replace("maxBytes: 10485760", "maxBytes: 4096")
        .replace("maxPixels: 40000000", "maxPixels: 10000")
        .replace("formats: [jpeg, png, webp]", "formats: [jpeg]");
    assert!(text.contains("maxBytes: 4096") && text.contains("formats: [jpeg]"));
    text.into_bytes()
}

/// `standard` with the document portrait's inter-eye distance relaxed to 40 px (the synthetic
/// ID cards print the portrait at about 56 px, below `standard`'s 60).
pub fn relaxed_profile() -> Vec<u8> {
    let line = "INTER_EYE_DISTANCE:      { enabled: true,  severity: error,   min: 60 }    # ICAO";
    let text = profile_text(STANDARD);
    assert!(
        text.contains(line),
        "standard.yaml document IED line changed"
    );
    text.replace("name: standard", "name: relaxed")
        .replace(
            line,
            "INTER_EYE_DISTANCE:      { enabled: true,  severity: error,   min: 40 }    # ICAO",
        )
        .into_bytes()
}

/// `standard` without selfie output settings.
pub fn no_output_profile() -> Vec<u8> {
    let block = "    enabled: true\n    style: portrait-3x4\n    width: 480\n    height: 640\n    format: jpeg\n    maxBytes: 102400\n    minJpegQuality: 60\n    background: keep\n";
    let text = profile_text(STANDARD);
    assert!(
        text.contains(block),
        "standard.yaml selfie output block changed"
    );
    text.replace("name: standard", "name: no-output")
        .replacen(block, "    enabled: false\n", 1)
        .into_bytes()
}

pub fn key(project: &str, key: &str, profiles: &[&str], default: Option<&str>) -> ApiKeyConfig {
    ApiKeyConfig {
        project: project.to_owned(),
        sha256: sha256_hex(key.as_bytes()),
        profiles: profiles.iter().map(|p| (*p).to_owned()).collect(),
        default_profile: default.map(str::to_owned),
        ..ApiKeyConfig::default()
    }
}

pub fn test_app(engine: Arc<dyn Engine>, settings: Settings) -> (Arc<App>, Router) {
    let profiles = ProfileStore::from_sources([
        ("standard.yaml".to_owned(), STANDARD.to_vec()),
        ("icao-strict.yaml".to_owned(), ICAO_STRICT.to_vec()),
        ("tiny.yaml".to_owned(), tiny_profile()),
        ("no-output.yaml".to_owned(), no_output_profile()),
        ("relaxed.yaml".to_owned(), relaxed_profile()),
    ])
    .unwrap();
    let keys = ApiKeys::from_config(
        &[
            key("all", KEY_ALL, &["*"], None),
            key("standard-only", KEY_STANDARD, &["standard"], None),
            key("tiny", KEY_TINY, &["tiny", "no-output"], Some("tiny")),
        ],
        &profiles,
    )
    .unwrap();
    let app = App::new(engine, profiles, keys, settings);
    app.set_ready(true);
    let router = api::router(app.clone());
    (app, router)
}

pub fn stub_app() -> (Arc<App>, Router) {
    test_app(Arc::new(StubEngine::new()), Settings::default())
}

/// A synthetic pattern (no faces).
pub fn pattern(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([((x * 7) % 255) as u8, ((y * 5) % 255) as u8, 128])
    })
}

pub fn jpeg(width: u32, height: u32) -> Vec<u8> {
    encode_jpeg(&pattern(width, height), 80).unwrap()
}

pub fn png(width: u32, height: u32) -> Vec<u8> {
    encode_png(&pattern(width, height)).unwrap()
}

// ------------------------------------------------------------------------------ requests

pub const BOUNDARY: &str = "veriface-test-boundary-7MA4YWxkTrZu0gW";

pub fn multipart_body(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, data) in parts {
        write!(
            body,
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{name}.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .unwrap();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    write!(body, "--{BOUNDARY}--\r\n").unwrap();
    body
}

pub fn post(uri: &str, key: Option<&str>, parts: &[(&str, &[u8])]) -> Request<Body> {
    let mut builder = Request::post(uri).header(
        header::CONTENT_TYPE,
        format!("multipart/form-data; boundary={BOUNDARY}"),
    );
    if let Some(key) = key {
        builder = builder.header("x-api-key", key);
    }
    builder.body(Body::from(multipart_body(parts))).unwrap()
}

pub fn get(uri: &str, key: Option<&str>) -> Request<Body> {
    let mut builder = Request::get(uri);
    if let Some(key) = key {
        builder = builder.header("x-api-key", key);
    }
    builder.body(Body::empty()).unwrap()
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "body is not JSON ({e}): {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn request_id(&self) -> &str {
        self.header("x-request-id")
            .expect("X-Request-Id is always set")
    }
}

pub async fn send(router: &Router, request: Request<Body>) -> Reply {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    Reply {
        status,
        headers,
        body,
    }
}

// ----------------------------------------------------------------- OpenAPI shape checking

pub struct Contract {
    doc: Value,
}

pub fn contract() -> &'static Contract {
    static CONTRACT: OnceLock<Contract> = OnceLock::new();
    CONTRACT.get_or_init(|| Contract {
        doc: serde_saphyr::from_str(OPENAPI).expect("contracts/openapi.yaml parses"),
    })
}

impl Contract {
    pub fn resolve<'a>(&'a self, mut schema: &'a Value) -> &'a Value {
        while let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            let pointer = reference.strip_prefix('#').expect("local $ref");
            schema = self
                .doc
                .pointer(pointer)
                .unwrap_or_else(|| panic!("unresolved {reference}"));
        }
        schema
    }

    pub fn schema(&self, name: &str) -> &Value {
        &self.doc["components"]["schemas"][name]
    }

    /// The schema of a documented response, or `None` when the status is not documented.
    pub fn response_schema(&self, path: &str, method: &str, status: StatusCode) -> Option<&Value> {
        let response = self.doc["paths"][path][method]["responses"].get(status.as_str())?;
        let content = self.resolve(response).get("content")?.as_object()?;
        content.values().next().map(|media| &media["schema"])
    }

    pub fn validate(&self, value: &Value, schema: &Value) -> Vec<String> {
        let mut errors = Vec::new();
        self.check(value, schema, "$", true, &mut errors);
        errors
    }

    /// Property names declared by `schema`, including through `allOf`.
    fn declared(&self, schema: &Value, names: &mut BTreeSet<String>, open: &mut Option<Value>) {
        let schema = self.resolve(schema);
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            names.extend(props.keys().cloned());
        }
        if let Some(additional) = schema.get("additionalProperties") {
            *open = Some(additional.clone());
        }
        for part in schema
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.declared(part, names, open);
        }
    }

    fn check(
        &self,
        value: &Value,
        schema: &Value,
        path: &str,
        closed: bool,
        errors: &mut Vec<String>,
    ) {
        let schema = self.resolve(schema);
        if let Some(expected) = schema.get("const")
            && value != expected
        {
            errors.push(format!("{path}: expected {expected}, got {value}"));
        }
        if let Some(options) = schema.get("enum").and_then(Value::as_array)
            && !options.contains(value)
        {
            errors.push(format!("{path}: {value} is not one of {options:?}"));
        }
        if let Some(types) = schema.get("type") {
            let types: Vec<&str> = match types {
                Value::String(t) => vec![t.as_str()],
                Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
                _ => vec![],
            };
            if !types.iter().any(|t| type_matches(value, t)) {
                errors.push(format!("{path}: {value} is not of type {types:?}"));
            }
        }
        for part in schema
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.check(value, part, path, false, errors);
        }
        if let Some(options) = schema.get("oneOf").and_then(Value::as_array) {
            let matching = options
                .iter()
                .filter(|o| {
                    let mut e = Vec::new();
                    self.check(value, o, path, true, &mut e);
                    e.is_empty()
                })
                .count();
            if matching != 1 {
                errors.push(format!("{path}: matches {matching} oneOf branches"));
            }
        }
        match value {
            Value::Object(map) => {
                for required in schema
                    .get("required")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if !map.contains_key(required) {
                        errors.push(format!("{path}: missing required `{required}`"));
                    }
                }
                if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                    for (name, sub) in props {
                        if let Some(v) = map.get(name) {
                            self.check(v, sub, &format!("{path}.{name}"), true, errors);
                        }
                    }
                }
                if closed {
                    let mut names = BTreeSet::new();
                    let mut open = None;
                    self.declared(schema, &mut names, &mut open);
                    if !names.is_empty() || open.is_some() {
                        for (name, v) in map {
                            if names.contains(name) {
                                continue;
                            }
                            match &open {
                                Some(additional) => self.check(
                                    v,
                                    additional,
                                    &format!("{path}.{name}"),
                                    true,
                                    errors,
                                ),
                                None => {
                                    errors.push(format!("{path}: unexpected property `{name}`"))
                                }
                            }
                        }
                    }
                }
            }
            Value::Array(items) => {
                if let Some(item_schema) = schema.get("items") {
                    for (i, item) in items.iter().enumerate() {
                        self.check(item, item_schema, &format!("{path}[{i}]"), true, errors);
                    }
                }
                let len = items.len() as u64;
                if schema
                    .get("minItems")
                    .and_then(Value::as_u64)
                    .is_some_and(|m| len < m)
                    || schema
                        .get("maxItems")
                        .and_then(Value::as_u64)
                        .is_some_and(|m| len > m)
                {
                    errors.push(format!("{path}: {len} items is outside minItems/maxItems"));
                }
            }
            Value::String(s)
                if schema
                    .get("maxLength")
                    .and_then(Value::as_u64)
                    .is_some_and(|m| s.chars().count() as u64 > m) =>
            {
                errors.push(format!("{path}: string longer than maxLength"));
            }
            _ => {}
        }
    }
}

pub fn type_matches(value: &Value, ty: &str) -> bool {
    match ty {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        other => panic!("unsupported schema type {other}"),
    }
}

/// Assert the reply matches the OpenAPI response for `path`/`method`/status (or the `Problem`
/// schema for undocumented error statuses), and return its JSON.
pub fn conforms(reply: &Reply, path: &str, method: &str) -> Value {
    let c = contract();
    let json = reply.json();
    let schema = match c.response_schema(path, method, reply.status) {
        Some(schema) => schema,
        None => {
            assert!(
                !reply.status.is_success(),
                "{method} {path} {} is not in the contract",
                reply.status
            );
            c.schema("Problem")
        }
    };
    let errors = c.validate(&json, schema);
    assert!(
        errors.is_empty(),
        "{method} {path} {} does not match the contract:\n{errors:#?}\n{json:#}",
        reply.status
    );
    json
}

/// Assert an RFC 9457 problem with `code` and `status`, consistent with the headers.
pub fn assert_problem(reply: &Reply, path: &str, method: &str, status: u16, code: &str) -> Value {
    assert_eq!(
        reply.status.as_u16(),
        status,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.header("content-type"),
        Some("application/problem+json")
    );
    let json = conforms(reply, path, method);
    assert_eq!(json["code"], code, "{json:#}");
    assert_eq!(json["status"], status);
    assert_eq!(
        json["type"],
        format!("https://ahmuhaisen.github.io/veriface/problems/{code}")
    );
    assert_eq!(json["instance"], reply.request_id());
    if status == 503 {
        assert!(
            reply.header("retry-after").is_some(),
            "503 needs Retry-After"
        );
    }
    json
}

// ------------------------------------------------------------------------ engine doubles

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    /// Block until the gate opens.
    Block,
    Panic,
    Fail,
    Unavailable,
}

pub struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
    entered: tokio::sync::mpsc::UnboundedSender<()>,
}

impl Gate {
    pub fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

/// Wraps the stub engine with a failure mode.
pub struct TestEngine {
    stub: StubEngine,
    mode: Mode,
    pub gate: Arc<Gate>,
}

impl TestEngine {
    pub fn new(mode: Mode) -> (Arc<Self>, tokio::sync::mpsc::UnboundedReceiver<()>) {
        let (entered, entered_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = TestEngine {
            stub: StubEngine::new(),
            mode,
            gate: Arc::new(Gate {
                open: Mutex::new(false),
                opened: Condvar::new(),
                entered,
            }),
        };
        (Arc::new(engine), entered_rx)
    }

    pub fn before(&self) -> Result<(), EngineError> {
        match self.mode {
            Mode::Block => {
                let _ = self.gate.entered.send(());
                let mut open = self.gate.open.lock().unwrap();
                while !*open {
                    open = self.gate.opened.wait(open).unwrap();
                }
                Ok(())
            }
            Mode::Panic => panic!("test engine panic"),
            Mode::Fail => Err(EngineError::Internal("test failure".into())),
            Mode::Unavailable => Err(EngineError::Unavailable("test outage".into())),
        }
    }
}

impl Engine for TestEngine {
    fn info(&self) -> &EngineInfo {
        self.stub.info()
    }

    fn warm_up(&self) -> Result<(), EngineError> {
        Ok(())
    }

    fn assess_quality(
        &self,
        request: &QualityRequest<'_>,
        stages: &mut Stages,
    ) -> Result<QualityBody, EngineError> {
        self.before()?;
        self.stub.assess_quality(request, stages)
    }

    fn extract_document_portrait(
        &self,
        request: &DocumentPortraitRequest<'_>,
        stages: &mut Stages,
    ) -> Result<DocumentPortraitBody, EngineError> {
        self.before()?;
        self.stub.extract_document_portrait(request, stages)
    }

    fn verify(
        &self,
        request: &VerificationRequest<'_>,
        stages: &mut Stages,
    ) -> Result<VerificationBody, EngineError> {
        self.before()?;
        self.stub.verify(request, stages)
    }
}
