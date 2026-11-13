//! RFC 9457 problem details (`application/problem+json`) with the codes listed in the `Problem`
//! schema of `contracts/openapi.yaml`.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use face_imaging::codec::DecodeError;
use serde::Serialize;

/// `type` is this prefix followed by the problem code.
pub const PROBLEM_TYPE_PREFIX: &str = "https://ahmuhaisen.github.io/veriface/problems/";
pub const PROBLEM_CONTENT_TYPE: &str = "application/problem+json";
/// Longest `detail` we send; longer ones are cut (they can echo client input).
const MAX_DETAIL_CHARS: usize = 512;

/// Every error the service returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProblemKind {
    InvalidRequest,
    Unauthorized,
    ProfileForbidden,
    ProfileNotFound,
    PayloadTooLarge,
    UnsupportedMediaType,
    ImageDecodeFailed,
    ImageTooLarge,
    Overloaded,
    NotReady,
    Internal,
    /// Unknown path.
    NotFound,
    /// Known path, wrong method.
    MethodNotAllowed,
    /// The API key's own request rate or concurrency limit (429). Proposed to the lead for the
    /// contract as `rate-limited`.
    RateLimited,
}

impl ProblemKind {
    pub const ALL: [ProblemKind; 14] = [
        ProblemKind::InvalidRequest,
        ProblemKind::Unauthorized,
        ProblemKind::ProfileForbidden,
        ProblemKind::ProfileNotFound,
        ProblemKind::PayloadTooLarge,
        ProblemKind::UnsupportedMediaType,
        ProblemKind::ImageDecodeFailed,
        ProblemKind::ImageTooLarge,
        ProblemKind::Overloaded,
        ProblemKind::NotReady,
        ProblemKind::Internal,
        ProblemKind::NotFound,
        ProblemKind::MethodNotAllowed,
        ProblemKind::RateLimited,
    ];

    /// The problem code, also the last segment of `type`.
    pub const fn code(self) -> &'static str {
        match self {
            ProblemKind::InvalidRequest => "invalid-request",
            ProblemKind::Unauthorized => "unauthorized",
            ProblemKind::ProfileForbidden => "profile-forbidden",
            ProblemKind::ProfileNotFound => "profile-not-found",
            ProblemKind::PayloadTooLarge => "payload-too-large",
            ProblemKind::UnsupportedMediaType => "unsupported-media-type",
            ProblemKind::ImageDecodeFailed => "image-decode-failed",
            ProblemKind::ImageTooLarge => "image-too-large",
            ProblemKind::Overloaded => "overloaded",
            ProblemKind::NotReady => "not-ready",
            ProblemKind::Internal => "internal",
            ProblemKind::NotFound => "not-found",
            ProblemKind::MethodNotAllowed => "method-not-allowed",
            ProblemKind::RateLimited => "rate-limited",
        }
    }

    pub const fn status(self) -> StatusCode {
        match self {
            ProblemKind::InvalidRequest => StatusCode::BAD_REQUEST,
            ProblemKind::Unauthorized => StatusCode::UNAUTHORIZED,
            ProblemKind::ProfileForbidden => StatusCode::FORBIDDEN,
            ProblemKind::ProfileNotFound | ProblemKind::NotFound => StatusCode::NOT_FOUND,
            ProblemKind::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            ProblemKind::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            ProblemKind::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ProblemKind::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ProblemKind::ImageDecodeFailed | ProblemKind::ImageTooLarge => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            ProblemKind::Overloaded | ProblemKind::NotReady => StatusCode::SERVICE_UNAVAILABLE,
            ProblemKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Short, stable English summary for developers. End-user text is localised by the
    /// integrating project from the code.
    pub const fn title(self) -> &'static str {
        match self {
            ProblemKind::InvalidRequest => "Invalid request",
            ProblemKind::Unauthorized => "Missing or invalid API key",
            ProblemKind::ProfileForbidden => "Profile not allowed for this API key",
            ProblemKind::ProfileNotFound => "Profile not found",
            ProblemKind::PayloadTooLarge => "Payload too large",
            ProblemKind::UnsupportedMediaType => "Unsupported media type",
            ProblemKind::ImageDecodeFailed => "Image could not be decoded",
            ProblemKind::ImageTooLarge => "Image has too many pixels",
            ProblemKind::Overloaded => "Service overloaded",
            ProblemKind::NotReady => "Service not ready",
            ProblemKind::Internal => "Internal error",
            ProblemKind::NotFound => "Not found",
            ProblemKind::MethodNotAllowed => "Method not allowed",
            ProblemKind::RateLimited => "Too many requests",
        }
    }

    /// Default `Retry-After` (seconds) for the 503 and 429 problems.
    pub const fn retry_after_secs(self) -> Option<u32> {
        match self {
            ProblemKind::Overloaded | ProblemKind::RateLimited => Some(1),
            ProblemKind::NotReady => Some(5),
            _ => None,
        }
    }
}

/// An error response. `instance` (the request id) is filled in by the request middleware.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub kind: ProblemKind,
    pub detail: Option<String>,
    /// Overrides the kind's default `Retry-After`.
    pub retry_after: Option<u32>,
}

impl Problem {
    pub fn new(kind: ProblemKind) -> Self {
        Problem {
            kind,
            detail: None,
            retry_after: None,
        }
    }

    pub fn with_detail(kind: ProblemKind, detail: impl Into<String>) -> Self {
        let mut detail: String = detail.into();
        if let Some((cut, _)) = detail.char_indices().nth(MAX_DETAIL_CHARS) {
            detail.truncate(cut);
            detail.push_str("...");
        }
        Problem {
            kind,
            detail: Some(detail),
            retry_after: None,
        }
    }

    /// The same problem with `Retry-After: seconds`.
    pub fn retry_after(mut self, seconds: u32) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    pub fn invalid_request(detail: impl Into<String>) -> Self {
        Problem::with_detail(ProblemKind::InvalidRequest, detail)
    }

    /// The JSON body.
    pub fn body(&self, instance: Option<&str>) -> Vec<u8> {
        #[derive(Serialize)]
        struct Body<'a> {
            #[serde(rename = "type")]
            type_uri: String,
            title: &'static str,
            status: u16,
            #[serde(skip_serializing_if = "Option::is_none")]
            detail: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            instance: Option<&'a str>,
            code: &'static str,
        }
        let body = Body {
            type_uri: format!("{PROBLEM_TYPE_PREFIX}{}", self.kind.code()),
            title: self.kind.title(),
            status: self.kind.status().as_u16(),
            detail: self.detail.as_deref(),
            instance,
            code: self.kind.code(),
        };
        // Serialising a struct of strings and integers cannot fail.
        serde_json::to_vec(&body).unwrap_or_default()
    }

    /// The full response, with `instance` set when known.
    pub fn to_response(&self, instance: Option<&str>) -> Response {
        let mut response = Response::new(Body::from(self.body(instance)));
        *response.status_mut() = self.kind.status();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(PROBLEM_CONTENT_TYPE),
        );
        if let Some(seconds) = self.retry_after.or(self.kind.retry_after_secs()) {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(seconds));
        }
        response
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let mut response = self.to_response(None);
        // The request middleware re-renders the body with `instance` and logs the code.
        response.extensions_mut().insert(self);
        response
    }
}

impl From<DecodeError> for Problem {
    /// The 1:1 mapping documented in `face_imaging::codec`.
    fn from(e: DecodeError) -> Self {
        let kind = match e {
            DecodeError::TooManyBytes { .. } => ProblemKind::PayloadTooLarge,
            DecodeError::UnsupportedFormat { .. } => ProblemKind::UnsupportedMediaType,
            DecodeError::TooManyPixels { .. } => ProblemKind::ImageTooLarge,
            DecodeError::Corrupt { .. } => ProblemKind::ImageDecodeFailed,
        };
        Problem::with_detail(kind, e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use face_imaging::codec::Format;

    #[test]
    fn decode_errors_map_one_to_one_to_problem_codes() {
        let cases = [
            (
                DecodeError::TooManyBytes { size: 2, limit: 1 },
                413,
                "payload-too-large",
            ),
            (
                DecodeError::UnsupportedFormat {
                    detected: Some(Format::Png),
                    reason: "png is not accepted here".into(),
                },
                415,
                "unsupported-media-type",
            ),
            (
                DecodeError::TooManyPixels {
                    width: 9,
                    height: 9,
                    limit: 80,
                },
                422,
                "image-too-large",
            ),
            (
                DecodeError::Corrupt { reason: "x".into() },
                422,
                "image-decode-failed",
            ),
        ];
        for (error, status, code) in cases {
            let problem = Problem::from(error);
            assert_eq!(problem.kind.status().as_u16(), status);
            assert_eq!(problem.kind.code(), code);
        }
    }

    #[test]
    fn renders_rfc9457_bodies() {
        let problem = Problem::with_detail(ProblemKind::Overloaded, "busy");
        let response = problem.to_response(Some("req-1"));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            PROBLEM_CONTENT_TYPE
        );
        assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        let body: serde_json::Value = serde_json::from_slice(&problem.body(Some("req-1"))).unwrap();
        assert_eq!(body["type"], "https://ahmuhaisen.github.io/veriface/problems/overloaded");
        assert_eq!(body["status"], 503);
        assert_eq!(body["code"], "overloaded");
        assert_eq!(body["instance"], "req-1");
        assert_eq!(body["detail"], "busy");
    }

    #[test]
    fn codes_are_unique_and_long_details_are_cut() {
        let mut codes: Vec<_> = ProblemKind::ALL.iter().map(|k| k.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ProblemKind::ALL.len());
        let long = Problem::invalid_request("x".repeat(5000));
        assert!(long.detail.unwrap().chars().count() <= MAX_DETAIL_CHARS + 3);
    }
}
