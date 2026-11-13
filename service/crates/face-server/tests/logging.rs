//! The per-request log line carries ids, statuses and timings only. In its own test binary:
//! a thread-scoped `tracing` subscriber races with tests running in parallel threads.

mod common;

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use serde_json::Value;

use common::*;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn request_log_lines_carry_ids_and_statuses_only() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let (_, router) = stub_app();
    let image = jpeg(16, 16);
    let mut request = post(
        "/v1/verifications",
        Some(KEY_ALL),
        &[("probe", &image), ("reference", &image)],
    );
    request
        .headers_mut()
        .insert("x-request-id", "log-check-1".parse().unwrap());
    assert_eq!(send(&router, request).await.status, StatusCode::OK);

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|l| l.contains("\"message\":\"request\""))
        .unwrap_or_else(|| panic!("no request log line in:\n{logs}"));
    let entry: Value = serde_json::from_str(line).unwrap();
    let fields = &entry["fields"];
    assert_eq!(fields["request_id"], "log-check-1", "{entry:#}");
    assert_eq!(fields["endpoint"], "/v1/verifications");
    assert_eq!(fields["status"], 200);
    assert_eq!(fields["outcome"], "review");
    assert_eq!(fields["project"], "all");
    assert!(
        fields["reasons"]
            .as_str()
            .unwrap()
            .contains("decision:NO_SCORE")
    );
    // Never the score, the image or check values.
    for forbidden in ["score", "\"value\"", "landmarks", "/9j/"] {
        assert!(
            !logs.contains(forbidden),
            "logs contain `{forbidden}`:\n{logs}"
        );
    }
}
