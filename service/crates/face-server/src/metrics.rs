//! Prometheus metrics (classic text format), named as in docs/architecture.md section 5.
//! Labels carry endpoints, statuses, stages, check ids and outcomes only: no biometric data.

use face_core::{Assessment, CheckResult, Outcome, Status};
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, IntGaugeVec, Opts, Registry,
    TextEncoder,
};

/// `Content-Type` of `GET /metrics`.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

const LATENCY_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];

pub struct Metrics {
    registry: Registry,
    /// `face_requests_total{endpoint, status}`.
    pub requests: IntCounterVec,
    /// `face_request_duration_seconds{endpoint}`.
    pub request_duration: HistogramVec,
    /// `face_stage_duration_seconds{stage}`.
    pub stage_duration: HistogramVec,
    /// `face_check_status_total{check, status}`.
    pub check_status: IntCounterVec,
    /// `face_verification_outcome_total{outcome}`.
    pub verification_outcome: IntCounterVec,
    /// `face_inflight`: requests holding a processing slot.
    pub inflight: IntGauge,
    /// `face_queue_depth`: requests waiting for a processing slot.
    pub queue_depth: IntGauge,
    /// `face_unread_body_total{outcome}`: request bodies the handler left unread (early
    /// rejections), by what happened to them: `drained`, or the connection was closed because
    /// of `too_large`, `busy`, `timeout`, `error`; `not_sent` (Expect: 100-continue, never sent).
    pub unread_body: IntCounterVec,
    /// `face_rate_limited_total{project, reason}`: `429 rate-limited` answers, by key project and
    /// `rate` or `concurrency`.
    pub rate_limited: IntCounterVec,
    /// `face_config_reloads_total{result}`: hot reloads of keys and profiles, `ok` or `error`.
    pub config_reloads: IntCounterVec,
    /// `face_open_connections{listener}`: open connections, `api` or `management`.
    pub open_connections: IntGaugeVec,
    /// `face_tls_handshake_errors_total{reason}`: failed TLS handshakes, `error` (protocol,
    /// certificate, client certificate refused) or `timeout`.
    pub tls_handshake_errors: IntCounterVec,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let requests = IntCounterVec::new(
            Opts::new(
                "face_requests_total",
                "HTTP requests by endpoint and status code",
            ),
            &["endpoint", "status"],
        )
        .expect("valid metric");
        let request_duration = HistogramVec::new(
            HistogramOpts::new(
                "face_request_duration_seconds",
                "Request latency by endpoint, from first byte to response",
            )
            .buckets(LATENCY_BUCKETS.to_vec()),
            &["endpoint"],
        )
        .expect("valid metric");
        let stage_duration = HistogramVec::new(
            HistogramOpts::new(
                "face_stage_duration_seconds",
                "Time per processing stage (queue, upload, decode, engine stages)",
            )
            .buckets(LATENCY_BUCKETS.to_vec()),
            &["stage"],
        )
        .expect("valid metric");
        let check_status = IntCounterVec::new(
            Opts::new(
                "face_check_status_total",
                "Check results by check id and status",
            ),
            &["check", "status"],
        )
        .expect("valid metric");
        let verification_outcome = IntCounterVec::new(
            Opts::new("face_verification_outcome_total", "Verification outcomes"),
            &["outcome"],
        )
        .expect("valid metric");
        let inflight = IntGauge::new("face_inflight", "Requests holding a processing slot")
            .expect("valid metric");
        let queue_depth =
            IntGauge::new("face_queue_depth", "Requests waiting for a processing slot")
                .expect("valid metric");
        let rate_limited = IntCounterVec::new(
            Opts::new(
                "face_rate_limited_total",
                "Requests refused by an API key's own rate or concurrency limit",
            ),
            &["project", "reason"],
        )
        .expect("valid metric");
        let config_reloads = IntCounterVec::new(
            Opts::new(
                "face_config_reloads_total",
                "Hot reloads of the API keys and profiles, by result",
            ),
            &["result"],
        )
        .expect("valid metric");
        let open_connections = IntGaugeVec::new(
            Opts::new(
                "face_open_connections",
                "Open client connections by listener",
            ),
            &["listener"],
        )
        .expect("valid metric");
        let tls_handshake_errors = IntCounterVec::new(
            Opts::new(
                "face_tls_handshake_errors_total",
                "Failed TLS handshakes (error: protocol or certificate; timeout)",
            ),
            &["reason"],
        )
        .expect("valid metric");
        let unread_body = IntCounterVec::new(
            Opts::new(
                "face_unread_body_total",
                "Request bodies left unread by an early response, by what happened to them",
            ),
            &["outcome"],
        )
        .expect("valid metric");

        for collector in [
            Box::new(requests.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(request_duration.clone()),
            Box::new(stage_duration.clone()),
            Box::new(check_status.clone()),
            Box::new(verification_outcome.clone()),
            Box::new(inflight.clone()),
            Box::new(queue_depth.clone()),
            Box::new(unread_body.clone()),
            Box::new(rate_limited.clone()),
            Box::new(config_reloads.clone()),
            Box::new(open_connections.clone()),
            Box::new(tls_handshake_errors.clone()),
        ] {
            registry
                .register(collector)
                .expect("metric names are unique");
        }

        Metrics {
            registry,
            requests,
            request_duration,
            stage_duration,
            check_status,
            verification_outcome,
            inflight,
            queue_depth,
            unread_body,
            rate_limited,
            config_reloads,
            open_connections,
            tls_handshake_errors,
        }
    }

    /// The text exposition.
    pub fn render(&self) -> String {
        let mut buffer = Vec::new();
        // Encoding gathered families into a Vec cannot fail.
        let _ = TextEncoder::new().encode(&self.registry.gather(), &mut buffer);
        String::from_utf8(buffer).unwrap_or_default()
    }

    pub fn observe_checks<'a>(&self, checks: impl IntoIterator<Item = &'a CheckResult>) {
        for check in checks {
            self.check_status
                .with_label_values(&[check.id.as_str(), status_label(check.status)])
                .inc();
        }
    }

    pub fn observe_assessment(&self, assessment: &Assessment) {
        self.observe_checks(&assessment.checks);
    }

    pub fn observe_outcome(&self, outcome: Outcome) {
        self.verification_outcome
            .with_label_values(&[outcome_label(outcome)])
            .inc();
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

pub const fn status_label(status: Status) -> &'static str {
    match status {
        Status::Pass => "pass",
        Status::Fail => "fail",
        Status::Undetermined => "undetermined",
        Status::Skipped => "skipped",
    }
}

pub const fn outcome_label(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Verified => "verified",
        Outcome::Rejected => "rejected",
        Outcome::Review => "review",
        Outcome::Retake => "retake",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_architecture_metric_names() {
        let m = Metrics::new();
        m.requests.with_label_values(&["/v1/quality", "200"]).inc();
        m.request_duration
            .with_label_values(&["/v1/quality"])
            .observe(0.2);
        m.stage_duration
            .with_label_values(&["decode"])
            .observe(0.01);
        m.check_status
            .with_label_values(&["FACE_PRESENT", "pass"])
            .inc();
        m.observe_outcome(Outcome::Review);
        m.inflight.set(2);
        let text = m.render();
        for name in [
            "face_requests_total{endpoint=\"/v1/quality\",status=\"200\"} 1",
            "face_request_duration_seconds_bucket",
            "face_stage_duration_seconds_count{stage=\"decode\"} 1",
            "face_check_status_total{check=\"FACE_PRESENT\",status=\"pass\"} 1",
            "face_verification_outcome_total{outcome=\"review\"} 1",
            "face_inflight 2",
            "face_queue_depth 0",
        ] {
            assert!(text.contains(name), "missing `{name}` in:\n{text}");
        }
    }
}
