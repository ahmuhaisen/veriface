//! The HTTP API of `contracts/openapi.yaml`: router, request middleware and handlers.
//!
//! Order of checks for the image endpoints: request id, API key (401), query (400), profile
//! (404, 403), readiness (503), content type (415), processing slot (503), upload limits (413,
//! 400), then decode (413, 415, 422) and the engine (500) on a blocking thread.

use std::net::{IpAddr, SocketAddr};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::multipart::{MultipartError, MultipartRejection};
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{
    ConnectInfo, DefaultBodyLimit, Extension, MatchedPath, Multipart, Path, Query, Request, State,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use face_core::{Assessment, OutputSettings, Profile, Verdict};
use face_imaging::codec::{self, DecodeError, Decoded};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use tracing::{Instrument, debug, error, info, warn};

use crate::auth::{self, ApiKey, ApiKeys};
use crate::config::IpRange;
use crate::dto::{DocumentPortraitResult, ProfileRef, QualityResult, Timings, VerificationResult};
use crate::engine::{
    DocumentPortraitRequest, Engine, EngineError, QualityRequest, ReferenceKind, Stages,
    VerificationRequest,
};
use crate::limiter::{Limiter, Overloaded, Permit};
use crate::metrics::{self, Metrics};
use crate::problem::{Problem, ProblemKind};
use crate::profiles::{LoadedProfile, ProfileStore};

pub const REQUEST_ID_HEADER: &str = "x-request-id";
/// Requests whose unread body may be drained at the same time (see [`drain_unread_body`]);
/// beyond this the connection is closed instead.
pub const MAX_CONCURRENT_DRAINS: usize = 64;
/// Least time a drain may take, even when the request deadline has already passed.
const MIN_DRAIN_TIME: Duration = Duration::from_secs(2);
/// Longest accepted `X-Request-Id` (OpenAPI `maxLength`).
pub const MAX_REQUEST_ID_LEN: usize = 128;
/// Allowance for multipart framing (boundaries, part headers) on top of the image bytes.
pub const MULTIPART_OVERHEAD: usize = 64 * 1024;

/// Runtime settings derived from the configuration.
#[derive(Clone, Debug)]
pub struct Settings {
    pub request_timeout: Duration,
    pub health_requires_key: bool,
    pub metrics_enabled: bool,
    pub max_concurrency: usize,
    pub queue_limit: usize,
    /// Serve `/metrics` on the API listener (false when a management listener serves it).
    pub metrics_on_api: bool,
    /// Proxies whose `X-Forwarded-For` names the client (see [`client_ip`]).
    pub trusted_proxies: Vec<IpRange>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            request_timeout: Duration::from_secs(30),
            health_requires_key: false,
            metrics_enabled: true,
            max_concurrency: 2,
            queue_limit: 8,
            metrics_on_api: true,
            trusted_proxies: Vec::new(),
        }
    }
}

/// The profiles and the API keys: what a hot reload replaces, always together, so a request
/// never sees keys that refer to profiles of another generation.
#[derive(Debug, Default)]
pub struct Catalog {
    pub profiles: ProfileStore,
    pub keys: ApiKeys,
}

/// Shared state of the running service.
pub struct App {
    pub engine: Arc<dyn Engine>,
    pub metrics: Metrics,
    pub limiter: Limiter,
    pub settings: Settings,
    catalog: RwLock<Arc<Catalog>>,
    /// Fixed at startup (the router's body limit); a reload may not raise it.
    body_limit: usize,
    ready: AtomicBool,
    drains: tokio::sync::Semaphore,
}

impl App {
    /// Not ready until [`App::set_ready`] (after the engine's warm-up).
    pub fn new(
        engine: Arc<dyn Engine>,
        profiles: ProfileStore,
        keys: ApiKeys,
        settings: Settings,
    ) -> Arc<App> {
        let metrics = Metrics::new();
        let limiter = Limiter::new(
            settings.max_concurrency,
            settings.queue_limit,
            metrics.inflight.clone(),
            metrics.queue_depth.clone(),
        );
        Arc::new(App {
            engine,
            metrics,
            limiter,
            settings,
            body_limit: App::body_limit_for(&profiles),
            catalog: RwLock::new(Arc::new(Catalog { profiles, keys })),
            ready: AtomicBool::new(false),
            drains: tokio::sync::Semaphore::new(MAX_CONCURRENT_DRAINS),
        })
    }

    /// The current profiles and keys. Requests keep the snapshot they took, so a reload never
    /// changes the rules under a running request.
    pub fn catalog(&self) -> Arc<Catalog> {
        self.catalog
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Swap in reloaded profiles and keys. Refused when the profiles would need a larger body
    /// limit than the one fixed at startup (that needs a restart).
    pub fn replace_catalog(&self, catalog: Catalog) -> Result<(), String> {
        let needed = App::body_limit_for(&catalog.profiles);
        if needed > self.body_limit {
            return Err(format!(
                "the reloaded profiles accept larger images (body limit {needed} bytes) than the \
                 running service ({} bytes); restart the service to apply them",
                self.body_limit
            ));
        }
        *self
            .catalog
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(catalog);
        Ok(())
    }

    fn body_limit_for(profiles: &ProfileStore) -> usize {
        profiles
            .max_input_bytes()
            .saturating_mul(2)
            .saturating_add(MULTIPART_OVERHEAD)
    }

    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::Release);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Whole-body limit: two images at the largest profile limit plus multipart framing. Each
    /// part is also checked against its profile's `input.maxBytes` while it streams in.
    pub fn body_limit(&self) -> usize {
        self.body_limit
    }

    /// The requested profile, else the key's default, else `standard`.
    fn resolve_profile(
        &self,
        key: &ApiKey,
        requested: Option<&str>,
        log: &RequestLog,
    ) -> Result<Arc<LoadedProfile>, Problem> {
        let name = requested.unwrap_or_else(|| key.default_profile());
        let catalog = self.catalog();
        let profile = catalog.profiles.get(name).ok_or_else(|| {
            Problem::with_detail(
                ProblemKind::ProfileNotFound,
                format!("no profile named `{}`", shorten(name)),
            )
        })?;
        if !key.allows(name) {
            return Err(Problem::with_detail(
                ProblemKind::ProfileForbidden,
                format!("this API key may not use profile `{name}`"),
            ));
        }
        log.update(|f| f.profile = Some(name.to_owned()));
        Ok(profile.clone())
    }

    /// Readiness, content type, a processing slot, then the upload itself.
    async fn receive(
        &self,
        headers: &HeaderMap,
        multipart: Result<Multipart, MultipartRejection>,
        profile: &LoadedProfile,
        names: &'static [&'static str],
    ) -> Result<Upload, Problem> {
        if !self.is_ready() {
            return Err(Problem::with_detail(
                ProblemKind::NotReady,
                "the engine is still warming up",
            ));
        }
        require_multipart(headers)?;
        let multipart = multipart.map_err(|e| {
            Problem::invalid_request(format!("invalid multipart/form-data: {}", e.body_text()))
        })?;
        let max_bytes = profile.decode_limits.max_bytes;
        if let Some(length) = content_length(headers) {
            let limit = max_bytes
                .saturating_mul(names.len())
                .saturating_add(MULTIPART_OVERHEAD);
            if length > limit as u64 {
                return Err(Problem::with_detail(
                    ProblemKind::PayloadTooLarge,
                    format!("the request body is {length} bytes; at most {limit} are accepted"),
                ));
            }
        }

        let mut stages = Stages::default();
        let queued = Instant::now();
        let permit = self.limiter.acquire().await.map_err(|Overloaded| {
            Problem::with_detail(
                ProblemKind::Overloaded,
                "every processing slot and queue place is taken",
            )
        })?;
        stages.record("queue", queued.elapsed());
        let uploading = Instant::now();
        let parts = read_parts(multipart, names, max_bytes).await?;
        stages.record("upload", uploading.elapsed());
        Ok(Upload {
            permit,
            parts,
            stages,
        })
    }

    /// Run `work` on the blocking pool. The processing slot moves into the task and is released
    /// when the work ends, even if the request has timed out meanwhile.
    async fn process<T, F>(&self, upload: Upload, work: F) -> Result<(T, Stages), Problem>
    where
        T: Send + 'static,
        F: FnOnce(&dyn Engine, &[Bytes], &mut Stages) -> Result<T, WorkError> + Send + 'static,
    {
        let Upload {
            permit,
            parts,
            mut stages,
        } = upload;
        let engine = self.engine.clone();
        let span = tracing::Span::current();
        let joined = tokio::task::spawn_blocking(move || {
            let _permit: Permit = permit;
            let _entered = span.enter();
            let result = work(engine.as_ref(), &parts, &mut stages);
            drop(parts); // free the uploaded bytes before the slot is released
            result.map(|value| (value, stages))
        })
        .await;
        match joined {
            Ok(Ok(done)) => Ok(done),
            Ok(Err(e)) => Err(e.into_problem()),
            Err(e) if e.is_panic() => {
                error!(
                    panic = %panic_message(e.into_panic().as_ref()),
                    "request processing panicked"
                );
                Err(Problem::new(ProblemKind::Internal))
            }
            Err(_) => {
                error!("request processing was cancelled");
                Err(Problem::new(ProblemKind::Internal))
            }
        }
    }

    fn finish_timings(&self, ctx: &RequestContext, stages: &Stages, log: &RequestLog) -> Timings {
        for (stage, elapsed) in stages.entries() {
            self.metrics
                .stage_duration
                .with_label_values(&[stage])
                .observe(elapsed.as_secs_f64());
        }
        let timings = Timings {
            total: ctx.start.elapsed(),
            stages: stages.entries().to_vec(),
        };
        log.update(|f| f.stages = timings.stages.clone());
        timings
    }
}

struct Upload {
    permit: Permit,
    parts: Vec<Bytes>,
    stages: Stages,
}

/// Failure inside the blocking work.
pub enum WorkError {
    Decode {
        part: &'static str,
        error: DecodeError,
    },
    Engine(EngineError),
}

impl WorkError {
    fn into_problem(self) -> Problem {
        match self {
            WorkError::Decode { part, error } => {
                let mut problem = Problem::from(error);
                problem.detail = problem.detail.map(|d| format!("part `{part}`: {d}"));
                problem
            }
            WorkError::Engine(EngineError::Unavailable(message)) => {
                error!(error = %message, "engine unavailable");
                Problem::with_detail(ProblemKind::NotReady, "the engine is unavailable")
            }
            WorkError::Engine(EngineError::Internal(message)) => {
                error!(error = %message, "engine failure");
                Problem::new(ProblemKind::Internal)
            }
        }
    }
}

fn decode_part(
    bytes: &[u8],
    part: &'static str,
    profile: &LoadedProfile,
    stages: &mut Stages,
) -> Result<Decoded, WorkError> {
    stages
        .time("decode", || codec::decode(bytes, &profile.decode_limits))
        .map_err(|error| WorkError::Decode { part, error })
}

// ------------------------------------------------------------------------- request context

/// Per-request values set by [`request_context`].
#[derive(Clone, Debug)]
pub struct RequestContext {
    pub request_id: Arc<str>,
    pub start: Instant,
}

/// What the per-request log line reports. Ids, statuses and timings only: never images,
/// embeddings, scores, check values or face geometry.
#[derive(Debug, Default)]
pub struct LogFields {
    pub project: Option<String>,
    /// The API key's `label`, when it has one.
    pub key: Option<String>,
    pub profile: Option<String>,
    pub verdict: Option<&'static str>,
    pub outcome: Option<&'static str>,
    pub decision: Option<&'static str>,
    /// Failed error-severity checks (or verification reason codes).
    pub reasons: Vec<String>,
    /// Failed warning-severity checks.
    pub warnings: Vec<String>,
    /// `CHECK: cause` for undetermined checks (debug level).
    pub undetermined: Vec<String>,
    pub stages: Vec<(&'static str, Duration)>,
}

impl LogFields {
    fn record_assessment(&mut self, assessment: &Assessment) {
        self.verdict = Some(verdict_label(assessment.verdict));
        self.reasons = assessment.reasons.iter().map(|id| id.to_string()).collect();
        self.warnings = assessment
            .warnings
            .iter()
            .map(|id| id.to_string())
            .collect();
        self.record_undetermined("", assessment);
    }

    fn record_undetermined(&mut self, prefix: &str, assessment: &Assessment) {
        self.undetermined
            .extend(assessment.checks.iter().filter_map(|c| {
                c.cause
                    .as_ref()
                    .map(|cause| format!("{prefix}{}: {cause}", c.id))
            }));
    }
}

/// Shared slot for [`LogFields`], filled by the handlers and middleware, logged at the end.
#[derive(Clone, Debug, Default)]
pub struct RequestLog(Arc<Mutex<LogFields>>);

impl RequestLog {
    pub fn update(&self, f: impl FnOnce(&mut LogFields)) {
        if let Ok(mut fields) = self.0.lock() {
            f(&mut fields);
        }
    }

    fn take(&self) -> LogFields {
        self.0
            .lock()
            .map(|mut fields| std::mem::take(&mut *fields))
            .unwrap_or_default()
    }
}

/// Outermost middleware: request id, deadline, panic guard, problem `instance`, metrics and the
/// per-request log line.
pub async fn request_context(
    State(app): State<Arc<App>>,
    mut request: Request,
    next: Next,
) -> Response {
    let start = Instant::now();
    let endpoint = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |p| p.as_str().to_owned());
    let method = request.method().clone();
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    let client = client_ip(peer, request.headers(), &app.settings.trusted_proxies);
    // Contract: an invalid X-Request-Id is replaced by a generated one, never an error.
    let request_id: Arc<str> = match request_id(request.headers()) {
        Ok(Some(id)) => Arc::from(id),
        Ok(None) => Arc::from(generate_request_id()),
        Err(reason) => {
            let generated = generate_request_id();
            debug!(request_id = %generated, reason, "X-Request-Id replaced");
            Arc::from(generated)
        }
    };
    let log = RequestLog::default();
    request.extensions_mut().insert(RequestContext {
        request_id: request_id.clone(),
        start,
    });
    request.extensions_mut().insert(log.clone());
    // Keep a handle on the body, so whatever the handler leaves unread can be drained below.
    let (parts, body) = request.into_parts();
    let unread = UnreadBody::new(&parts.headers, body);
    let request = Request::from_parts(parts, Body::from_stream(unread.clone()));

    let span = tracing::info_span!("request", request_id = %request_id, endpoint = %endpoint);
    let guarded = AssertUnwindSafe(next.run(request)).catch_unwind();
    let response = match tokio::time::timeout(app.settings.request_timeout, guarded)
        .instrument(span.clone())
        .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(panic)) => {
            span.in_scope(|| {
                error!(panic = %panic_message(panic.as_ref()), "request handler panicked");
            });
            Problem::new(ProblemKind::Internal).into_response()
        }
        Err(_) => Problem::with_detail(
            ProblemKind::Overloaded,
            format!(
                "the request did not complete within {} ms",
                app.settings.request_timeout.as_millis()
            ),
        )
        .into_response(),
    };
    let remaining = app.settings.request_timeout.saturating_sub(start.elapsed());
    let drained = drain_unread_body(&app, &unread, remaining.max(MIN_DRAIN_TIME))
        .instrument(span.clone())
        .await;
    let mut response = finalize(response, &request_id);
    if !drained {
        // Tell the client the connection ends with this response.
        response
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
    }

    let status = response.status();
    let elapsed = start.elapsed();
    app.metrics
        .requests
        .with_label_values(&[endpoint.as_str(), status.as_str()])
        .inc();
    app.metrics
        .request_duration
        .with_label_values(&[endpoint.as_str()])
        .observe(elapsed.as_secs_f64());
    span.in_scope(|| {
        log_request(
            &request_id,
            client,
            &method,
            &endpoint,
            &response,
            elapsed,
            log.take(),
        );
    });
    response
}

/// Fill in the problem `instance`, normalise stray non-problem errors, add `X-Request-Id`.
fn finalize(mut response: Response, request_id: &str) -> Response {
    if let Some(problem) = response.extensions().get::<Problem>() {
        *response.body_mut() = Body::from(problem.body(Some(request_id)));
    } else if response.status().is_client_error() || response.status().is_server_error() {
        // Every error is a problem document, even ones raised outside our handlers.
        let kind = match response.status() {
            StatusCode::UNAUTHORIZED => ProblemKind::Unauthorized,
            StatusCode::NOT_FOUND => ProblemKind::NotFound,
            StatusCode::METHOD_NOT_ALLOWED => ProblemKind::MethodNotAllowed,
            StatusCode::TOO_MANY_REQUESTS => ProblemKind::RateLimited,
            StatusCode::PAYLOAD_TOO_LARGE => ProblemKind::PayloadTooLarge,
            StatusCode::UNSUPPORTED_MEDIA_TYPE => ProblemKind::UnsupportedMediaType,
            s if s.is_client_error() => ProblemKind::InvalidRequest,
            _ => ProblemKind::Internal,
        };
        warn!(status = %response.status(), "unmapped error response replaced by a problem");
        let problem = Problem::new(kind);
        let mut replaced = problem.to_response(Some(request_id));
        replaced.extensions_mut().insert(problem);
        response = replaced;
    }
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static(REQUEST_ID_HEADER), value);
    }
    response
}

fn log_request(
    request_id: &str,
    client: Option<IpAddr>,
    method: &axum::http::Method,
    endpoint: &str,
    response: &Response,
    elapsed: Duration,
    fields: LogFields,
) {
    let status = response.status().as_u16();
    let problem = response.extensions().get::<Problem>();
    let problem_code = problem.map(|p| p.kind.code());
    let problem_detail = problem.and_then(|p| p.detail.as_deref());
    let join = |v: &[String]| (!v.is_empty()).then(|| v.join(","));
    let reasons = join(&fields.reasons);
    let warnings = join(&fields.warnings);
    let stages = (!fields.stages.is_empty()).then(|| {
        fields
            .stages
            .iter()
            .map(|(stage, d)| format!("{stage}={:.1}", d.as_secs_f64() * 1000.0))
            .collect::<Vec<_>>()
            .join(",")
    });
    let duration_ms = (elapsed.as_secs_f64() * 1_000_000.0).round() / 1000.0;
    macro_rules! emit {
        ($level:ident) => {
            $level!(
                request_id,
                client_ip = client.map(tracing::field::display),
                method = %method,
                endpoint,
                status,
                duration_ms,
                project = fields.project.as_deref(),
                key = fields.key.as_deref(),
                profile = fields.profile.as_deref(),
                verdict = fields.verdict,
                outcome = fields.outcome,
                decision = fields.decision,
                reasons = reasons.as_deref(),
                warnings = warnings.as_deref(),
                stages_ms = stages.as_deref(),
                problem = problem_code,
                detail = problem_detail,
                "request"
            )
        };
    }
    if status >= 500 && status != 503 {
        emit!(error);
    } else if status == 503 {
        emit!(warn);
    } else {
        emit!(info);
    }
    if !fields.undetermined.is_empty() {
        debug!(undetermined = %fields.undetermined.join("; "), "undetermined checks");
    }
}

/// `Ok(None)` when absent; `Err(reason)` when invalid (more than one, empty, too long, or not
/// visible ASCII), in which case the caller generates a replacement.
fn request_id(headers: &HeaderMap) -> Result<Option<String>, &'static str> {
    let mut values = headers.get_all(REQUEST_ID_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err("more than one X-Request-Id header");
    }
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_REQUEST_ID_LEN {
        return Err("X-Request-Id is empty or longer than 128 characters");
    }
    if !bytes.iter().all(|b| (0x21..=0x7E).contains(b)) {
        return Err("X-Request-Id has characters other than visible ASCII");
    }
    // Checked above: visible ASCII.
    Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
}

/// The client's address: the peer, or, when the peer is a trusted proxy, the right-most address
/// in `X-Forwarded-For` that is not itself a trusted proxy. Only for the logs; never used for a
/// security decision.
pub fn client_ip(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &[IpRange]) -> Option<IpAddr> {
    let peer = peer?;
    let is_trusted = |ip: IpAddr| trusted.iter().any(|range| range.contains(ip));
    if !is_trusted(peer) {
        return Some(peer);
    }
    let forwarded: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map_while(|hop| hop.trim().parse().ok())
        .collect();
    forwarded
        .iter()
        .rev()
        .copied()
        .find(|ip| !is_trusted(*ip))
        .or(forwarded.first().copied())
        .or(Some(peer))
}

fn generate_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

// ------------------------------------------------------------------------- unread bodies

/// The request body, shared between the handler (which reads it through a stream) and
/// [`request_context`] (which drains what the handler left unread).
#[derive(Clone)]
struct UnreadBody(Arc<Mutex<BodyState>>);

struct BodyState {
    stream: axum::body::BodyDataStream,
    /// Bytes read so far, by the handler and the drain.
    read: u64,
    /// The stream has been polled: hyper has sent `100 Continue` if the client asked for it.
    polled: bool,
    /// The stream ended (or failed).
    done: bool,
    declared: Option<u64>,
    expects_continue: bool,
}

impl UnreadBody {
    fn new(headers: &HeaderMap, body: Body) -> Self {
        let expects_continue = headers
            .get(header::EXPECT)
            .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"100-continue"));
        UnreadBody(Arc::new(Mutex::new(BodyState {
            stream: body.into_data_stream(),
            read: 0,
            polled: false,
            done: false,
            declared: content_length(headers),
            expects_continue,
        })))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, BodyState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl futures_util::Stream for UnreadBody {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use futures_util::StreamExt;
        use std::task::Poll;
        let mut state = self.state();
        if state.done {
            return Poll::Ready(None);
        }
        state.polled = true;
        let next = state.stream.poll_next_unpin(cx);
        match &next {
            Poll::Ready(Some(Ok(chunk))) => state.read += chunk.len() as u64,
            Poll::Ready(Some(Err(_)) | None) => state.done = true,
            Poll::Pending => {}
        }
        next
    }
}

/// Read and discard the part of the request body the handler did not read, so that a client
/// which sends its whole upload before reading the response (most HTTP clients without
/// `Expect: 100-continue`) receives the early problem response (401, 403, 404, 413, 415, 503,
/// ...) instead of a connection reset. `true` when the body was read completely.
///
/// Bounds, because the client may be unauthenticated:
/// - never more than the service's whole-body limit in total (a larger declared
///   `Content-Length` is not drained at all);
/// - at most `MAX_CONCURRENT_DRAINS` drains at once, without a processing slot or a buffer
///   (chunks are dropped as they arrive);
/// - at most the rest of the request deadline (at least `MIN_DRAIN_TIME`).
///
/// A client that sent `Expect: 100-continue` and was refused before the handler read anything
/// has not sent its body: nothing to drain. Otherwise the response says `Connection: close`
/// and hyper closes the connection.
async fn drain_unread_body(app: &App, body: &UnreadBody, time_limit: Duration) -> bool {
    use futures_util::StreamExt;
    let cap = app.body_limit() as u64;
    let outcome = {
        let state = body.state();
        if state.done {
            return true;
        }
        if state.expects_continue && !state.polled {
            "not_sent"
        } else if state.declared.is_some_and(|length| length > cap) {
            "too_large"
        } else {
            ""
        }
    };
    let outcome = if !outcome.is_empty() {
        outcome
    } else if let Ok(_permit) = app.drains.try_acquire() {
        let mut stream = body.clone();
        let drain = async {
            while let Some(chunk) = stream.next().await {
                if chunk.is_err() {
                    return "error";
                }
                if body.state().read > cap {
                    return "too_large";
                }
            }
            "drained"
        };
        tokio::time::timeout(time_limit, drain)
            .await
            .unwrap_or("timeout")
    } else {
        "busy"
    };
    app.metrics.unread_body.with_label_values(&[outcome]).inc();
    let read = body.state().read;
    debug!(outcome, read_bytes = read, "unread request body");
    outcome == "drained"
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

// --------------------------------------------------------------------------------- router

/// The service's router, with all middleware.
pub fn router(app: Arc<App>) -> Router {
    let v1 = Router::new()
        .route("/v1/quality", post(assess_quality))
        .route("/v1/document-portraits", post(extract_document_portrait))
        .route("/v1/verifications", post(verify))
        .route("/v1/profiles", get(list_profiles))
        .route("/v1/profiles/{name}", get(get_profile))
        .route("/v1/info", get(info))
        .route_layer(middleware::from_fn_with_state(
            app.clone(),
            auth::require_api_key,
        ));
    let mut health = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready));
    if app.settings.health_requires_key {
        health = health.route_layer(middleware::from_fn_with_state(
            app.clone(),
            auth::require_api_key,
        ));
    }
    let mut router = Router::new().merge(v1).merge(health);
    if app.settings.metrics_on_api {
        router = router.route("/metrics", get(metrics_text));
    }
    router
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(app.body_limit()))
        .layer(middleware::from_fn_with_state(app.clone(), request_context))
        .with_state(app)
}

/// The management listener (`server.management_bind`): health and metrics only, no API key,
/// no per-request log lines (probes would flood them).
pub fn management_router(app: Arc<App>) -> Router {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/metrics", get(metrics_text))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(MULTIPART_OVERHEAD))
        .with_state(app)
}

// ------------------------------------------------------------------------------- handlers

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssessQuery {
    profile: Option<String>,
    output: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct VerifyQuery {
    profile: Option<String>,
    reference_kind: Option<ReferenceKind>,
    operating_point: Option<String>,
}

async fn assess_quality(
    State(app): State<Arc<App>>,
    Extension(key): Extension<Arc<ApiKey>>,
    Extension(ctx): Extension<RequestContext>,
    Extension(log): Extension<RequestLog>,
    headers: HeaderMap,
    query: Result<Query<AssessQuery>, QueryRejection>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Response, Problem> {
    let Query(query) = query.map_err(query_problem)?;
    let profile = app.resolve_profile(&key, query.profile.as_deref(), &log)?;
    let include_output = include_output(query.output, &profile.profile.selfie.output, "selfie")?;
    let upload = app
        .receive(&headers, multipart, &profile, &["image"])
        .await?;

    let work_profile = profile.clone();
    let (body, stages) = app
        .process(upload, move |engine, parts, stages| {
            let image = decode_part(&parts[0], "image", &work_profile, stages)?;
            let request = QualityRequest {
                profile: &work_profile.profile,
                image: &image,
                include_output,
            };
            engine
                .assess_quality(&request, stages)
                .map_err(WorkError::Engine)
        })
        .await?;

    app.metrics.observe_assessment(&body.subject.assessment);
    log.update(|f| f.record_assessment(&body.subject.assessment));
    let timings = app.finish_timings(&ctx, &stages, &log);
    Ok(json(
        StatusCode::OK,
        &QualityResult {
            request_id: &ctx.request_id,
            subject: "selfie",
            body: &body,
            profile: &profile.reference,
            engine: app.engine.info(),
            timings: &timings,
        },
    ))
}

async fn extract_document_portrait(
    State(app): State<Arc<App>>,
    Extension(key): Extension<Arc<ApiKey>>,
    Extension(ctx): Extension<RequestContext>,
    Extension(log): Extension<RequestLog>,
    headers: HeaderMap,
    query: Result<Query<AssessQuery>, QueryRejection>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Response, Problem> {
    let Query(query) = query.map_err(query_problem)?;
    let profile = app.resolve_profile(&key, query.profile.as_deref(), &log)?;
    let include_output =
        include_output(query.output, &profile.profile.document.output, "document")?;
    let upload = app
        .receive(&headers, multipart, &profile, &["document"])
        .await?;

    let work_profile = profile.clone();
    let (body, stages) = app
        .process(upload, move |engine, parts, stages| {
            let document = decode_part(&parts[0], "document", &work_profile, stages)?;
            let request = DocumentPortraitRequest {
                profile: &work_profile.profile,
                document: &document,
                include_output,
            };
            engine
                .extract_document_portrait(&request, stages)
                .map_err(WorkError::Engine)
        })
        .await?;

    app.metrics.observe_assessment(&body.subject.assessment);
    log.update(|f| f.record_assessment(&body.subject.assessment));
    let timings = app.finish_timings(&ctx, &stages, &log);
    Ok(json(
        StatusCode::OK,
        &DocumentPortraitResult {
            request_id: &ctx.request_id,
            subject: "document",
            body: &body,
            profile: &profile.reference,
            engine: app.engine.info(),
            timings: &timings,
        },
    ))
}

async fn verify(
    State(app): State<Arc<App>>,
    Extension(key): Extension<Arc<ApiKey>>,
    Extension(ctx): Extension<RequestContext>,
    Extension(log): Extension<RequestLog>,
    headers: HeaderMap,
    query: Result<Query<VerifyQuery>, QueryRejection>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Response, Problem> {
    let Query(query) = query.map_err(query_problem)?;
    let profile = app.resolve_profile(&key, query.profile.as_deref(), &log)?;
    let verification = &profile.profile.verification;
    let operating_point = verification
        .operating_point(query.operating_point.as_deref())
        .cloned()
        .ok_or_else(|| {
            let known: Vec<&str> = verification
                .operating_points
                .iter()
                .map(|op| op.id.as_str())
                .collect();
            Problem::invalid_request(format!(
                "unknown operatingPoint `{}`; profile `{}` defines: {}",
                shorten(query.operating_point.as_deref().unwrap_or_default()),
                profile.name(),
                known.join(", ")
            ))
        })?;
    let reference_kind = query.reference_kind.unwrap_or_default();
    let upload = app
        .receive(&headers, multipart, &profile, &["probe", "reference"])
        .await?;

    let work_profile = profile.clone();
    let (body, stages) = app
        .process(upload, move |engine, parts, stages| {
            let probe = decode_part(&parts[0], "probe", &work_profile, stages)?;
            let reference = decode_part(&parts[1], "reference", &work_profile, stages)?;
            let request = VerificationRequest {
                profile: &work_profile.profile,
                operating_point: &operating_point,
                reference_kind,
                probe: &probe,
                reference: &reference,
            };
            engine.verify(&request, stages).map_err(WorkError::Engine)
        })
        .await?;

    app.metrics.observe_assessment(&body.probe.assessment);
    app.metrics
        .observe_assessment(&body.reference.subject.assessment);
    app.metrics.observe_checks(&body.risk.checks);
    app.metrics.observe_outcome(body.outcome);
    log.update(|f| {
        f.outcome = Some(metrics::outcome_label(body.outcome));
        f.decision = Some(decision_label(body.decision));
        f.reasons = body
            .reasons
            .iter()
            .map(|r| format!("{}:{}", reason_source_label(r.source), r.code))
            .collect();
        f.record_undetermined("probe.", &body.probe.assessment);
        f.record_undetermined("reference.", &body.reference.subject.assessment);
    });
    let timings = app.finish_timings(&ctx, &stages, &log);
    Ok(json(
        StatusCode::OK,
        &VerificationResult {
            request_id: &ctx.request_id,
            body: &body,
            profile: &profile.reference,
            engine: app.engine.info(),
            timings: &timings,
        },
    ))
}

#[derive(Serialize)]
struct ProfileList<'a> {
    profiles: Vec<&'a ProfileRef>,
}

async fn list_profiles(
    State(app): State<Arc<App>>,
    Extension(key): Extension<Arc<ApiKey>>,
) -> Response {
    let catalog = app.catalog();
    let profiles = catalog
        .profiles
        .iter()
        .filter(|p| key.allows(p.name()))
        .map(|p| &p.reference)
        .collect();
    json(StatusCode::OK, &ProfileList { profiles })
}

#[derive(Serialize)]
struct ProfileDocument<'a> {
    sha256: &'a str,
    profile: &'a Profile,
}

async fn get_profile(
    State(app): State<Arc<App>>,
    Extension(key): Extension<Arc<ApiKey>>,
    Extension(log): Extension<RequestLog>,
    name: Result<Path<String>, PathRejection>,
) -> Result<Response, Problem> {
    let Path(name) = name.map_err(|_| Problem::new(ProblemKind::ProfileNotFound))?;
    let profile = app.resolve_profile(&key, Some(&name), &log)?;
    Ok(json(
        StatusCode::OK,
        &ProfileDocument {
            sha256: &profile.reference.sha256,
            profile: &profile.profile,
        },
    ))
}

async fn info(State(app): State<Arc<App>>) -> Response {
    json(StatusCode::OK, app.engine.info())
}

#[derive(Serialize)]
struct HealthStatus {
    status: &'static str,
}

async fn live() -> Response {
    json(StatusCode::OK, &HealthStatus { status: "live" })
}

async fn ready(State(app): State<Arc<App>>) -> Response {
    if app.is_ready() {
        json(StatusCode::OK, &HealthStatus { status: "ready" })
    } else {
        Problem::with_detail(ProblemKind::NotReady, "the engine is not ready").into_response()
    }
}

async fn metrics_text(State(app): State<Arc<App>>) -> Response {
    if !app.settings.metrics_enabled {
        return Problem::new(ProblemKind::NotFound).into_response();
    }
    (
        [(header::CONTENT_TYPE, metrics::CONTENT_TYPE)],
        app.metrics.render(),
    )
        .into_response()
}

async fn not_found() -> Problem {
    Problem::new(ProblemKind::NotFound)
}

async fn method_not_allowed() -> Problem {
    Problem::new(ProblemKind::MethodNotAllowed)
}

// -------------------------------------------------------------------------------- helpers

fn json<T: Serialize + ?Sized>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = status;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
        }
        Err(e) => {
            error!(error = %e, "response serialisation failed");
            Problem::new(ProblemKind::Internal).into_response()
        }
    }
}

fn query_problem(e: QueryRejection) -> Problem {
    Problem::invalid_request(format!("invalid query string: {}", e.body_text()))
}

fn require_multipart(headers: &HeaderMap) -> Result<(), Problem> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    if essence.eq_ignore_ascii_case("multipart/form-data") {
        Ok(())
    } else {
        Err(Problem::with_detail(
            ProblemKind::UnsupportedMediaType,
            "send the images as multipart/form-data",
        ))
    }
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

fn include_output(
    requested: Option<bool>,
    output: &OutputSettings,
    section: &str,
) -> Result<bool, Problem> {
    let include = requested.unwrap_or(output.enabled);
    let complete = output.style.is_some()
        && output.width.is_some()
        && output.height.is_some()
        && output.format.is_some();
    if include && !complete {
        return Err(Problem::invalid_request(format!(
            "output was requested, but the profile has no complete {section}.output settings"
        )));
    }
    Ok(include)
}

/// Read exactly the named parts, each at most `max_bytes`; reject unknown, duplicate, empty and
/// missing parts. Part Content-Types are ignored: formats are sniffed from the bytes.
async fn read_parts(
    mut multipart: Multipart,
    names: &[&'static str],
    max_bytes: usize,
) -> Result<Vec<Bytes>, Problem> {
    let mut parts: Vec<Option<Bytes>> = vec![None; names.len()];
    let expected = names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(" and ");
    while let Some(mut field) = multipart.next_field().await.map_err(multipart_problem)? {
        let Some(name) = field.name().map(str::to_owned) else {
            return Err(Problem::invalid_request(format!(
                "a part has no name; send {expected}"
            )));
        };
        let Some(index) = names.iter().position(|n| *n == name) else {
            return Err(Problem::invalid_request(format!(
                "unexpected part `{}`; send {expected}",
                shorten(&name)
            )));
        };
        if parts[index].is_some() {
            return Err(Problem::invalid_request(format!(
                "part `{name}` was sent more than once"
            )));
        }
        let mut data: Vec<u8> = Vec::new();
        while let Some(chunk) = field.chunk().await.map_err(multipart_problem)? {
            if data.len() + chunk.len() > max_bytes {
                return Err(Problem::with_detail(
                    ProblemKind::PayloadTooLarge,
                    format!("part `{name}` is larger than {max_bytes} bytes"),
                ));
            }
            data.extend_from_slice(&chunk);
        }
        if data.is_empty() {
            return Err(Problem::invalid_request(format!("part `{name}` is empty")));
        }
        parts[index] = Some(Bytes::from(data));
    }
    let missing: Vec<&str> = names
        .iter()
        .zip(&parts)
        .filter(|(_, part)| part.is_none())
        .map(|(name, _)| *name)
        .collect();
    if !missing.is_empty() {
        return Err(Problem::invalid_request(format!(
            "missing part {}; send {expected}",
            missing
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(" and ")
        )));
    }
    Ok(parts.into_iter().flatten().collect())
}

fn multipart_problem(e: MultipartError) -> Problem {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        Problem::with_detail(
            ProblemKind::PayloadTooLarge,
            "the request body is too large",
        )
    } else {
        Problem::invalid_request(format!("malformed multipart body: {}", e.body_text()))
    }
}

/// Client-supplied text in error details is cut to 64 characters.
fn shorten(s: &str) -> String {
    match s.char_indices().nth(64) {
        Some((cut, _)) => format!("{}...", &s[..cut]),
        None => s.to_owned(),
    }
}

const fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Accepted => "accepted",
        Verdict::Rejected => "rejected",
        Verdict::Undetermined => "undetermined",
    }
}

const fn decision_label(decision: face_core::Decision) -> &'static str {
    match decision {
        face_core::Decision::Match => "match",
        face_core::Decision::NoMatch => "no_match",
        face_core::Decision::Undetermined => "undetermined",
    }
}

const fn reason_source_label(source: face_core::ReasonSource) -> &'static str {
    match source {
        face_core::ReasonSource::Probe => "probe",
        face_core::ReasonSource::Reference => "reference",
        face_core::ReasonSource::Risk => "risk",
        face_core::ReasonSource::Decision => "decision",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_are_validated() {
        let mut headers = HeaderMap::new();
        assert_eq!(request_id(&headers), Ok(None));
        headers.insert(REQUEST_ID_HEADER, HeaderValue::from_static("abc-123"));
        assert_eq!(request_id(&headers), Ok(Some("abc-123".to_owned())));
        headers.insert(
            REQUEST_ID_HEADER,
            HeaderValue::from_str(&"x".repeat(129)).unwrap(),
        );
        assert!(request_id(&headers).is_err());
        headers.insert(REQUEST_ID_HEADER, HeaderValue::from_static("has space"));
        assert!(request_id(&headers).is_err());
        headers.insert(REQUEST_ID_HEADER, HeaderValue::from_static("a"));
        headers.append(REQUEST_ID_HEADER, HeaderValue::from_static("b"));
        assert!(request_id(&headers).is_err());
        assert_eq!(generate_request_id().len(), 36);
    }

    #[test]
    fn client_addresses_come_from_trusted_proxies_only() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let trusted = [IpRange::parse("10.0.0.0/8").unwrap()];
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.9, 10.1.1.1"),
        );
        // An untrusted peer's header is ignored.
        assert_eq!(
            client_ip(Some(ip("198.51.100.1")), &headers, &trusted),
            Some(ip("198.51.100.1"))
        );
        // Behind a trusted proxy: the right-most untrusted hop.
        assert_eq!(
            client_ip(Some(ip("10.0.0.5")), &headers, &trusted),
            Some(ip("203.0.113.9"))
        );
        // No header: the proxy itself.
        assert_eq!(
            client_ip(Some(ip("10.0.0.5")), &HeaderMap::new(), &trusted),
            Some(ip("10.0.0.5"))
        );
        // IPv4-mapped IPv6 peers match IPv4 ranges.
        assert_eq!(
            client_ip(Some(ip("::ffff:10.0.0.5")), &headers, &trusted),
            Some(ip("203.0.113.9"))
        );
        assert_eq!(client_ip(None, &headers, &trusted), None);
    }

    #[test]
    fn shortens_client_text() {
        assert_eq!(shorten("abc"), "abc");
        assert_eq!(shorten(&"é".repeat(100)).chars().count(), 67);
    }
}
