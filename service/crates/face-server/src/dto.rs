//! Response bodies, shaped exactly like the schemas in `contracts/openapi.yaml`.
//!
//! The engine produces the `*Body` types (everything except `requestId`, `profile`, `engine` and
//! `timings`); the server wraps them into the `*Result` types. Nullable fields are always
//! present (as `null`), never omitted.

use std::fmt;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use face_core::{
    Assessment, CalibrationStatus, Catalogue, CheckResult, Decision, OperatingPoint, Outcome,
    Profile, Reason, VerificationInput,
};
use face_imaging::geom::{Point, Rect};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

// ------------------------------------------------------------------------- shared shapes

/// `ProfileRef`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRef {
    pub name: String,
    pub version: String,
    /// Hex SHA-256 of the profile file's bytes.
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `EngineInfo`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub version: String,
    pub catalogue_version: u32,
    pub models: Vec<ModelInfo>,
}

/// One entry of `EngineInfo.models`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub role: String,
    pub sha256: String,
    pub review_status: ReviewStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewStatus {
    Approved,
    Pending,
    Rejected,
}

/// `Point`: `[x, y]` in pixels.
pub type PointDto = [f64; 2];

pub fn point(p: Point) -> PointDto {
    [f64::from(p.x), f64::from(p.y)]
}

/// `Box` (renamed: `Box` is a Rust prelude type).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<Rect> for BoundingBox {
    fn from(r: Rect) -> Self {
        BoundingBox {
            x: f64::from(r.x),
            y: f64::from(r.y),
            width: f64::from(r.width),
            height: f64::from(r.height),
        }
    }
}

/// `FaceInfo`. Biometric geometry: returned to the caller, never logged.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceInfo {
    #[serde(rename = "box")]
    pub bounding_box: BoundingBox,
    /// Subject right eye centre, subject left eye centre, nose tip, right mouth corner, left
    /// mouth corner (the subject's own left/right).
    pub landmarks5: [PointDto; 5],
    pub inter_eye_distance_px: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
}

impl fmt::Debug for FaceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FaceInfo { .. }")
    }
}

/// Head pose in degrees, sign conventions as in `contracts/checks.yaml`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Pose {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum OutputMediaType {
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/png")]
    Png,
}

/// `OutputImage`. `bytes` and the base64 `data` are derived from [`OutputImage::data`].
#[derive(Clone, PartialEq)]
pub struct OutputImage {
    pub media_type: OutputMediaType,
    pub width: u32,
    pub height: u32,
    /// The encoded file (JPEG or PNG).
    pub data: Vec<u8>,
}

impl Serialize for OutputImage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(5))?;
        map.serialize_entry("mediaType", &self.media_type)?;
        map.serialize_entry("width", &self.width)?;
        map.serialize_entry("height", &self.height)?;
        map.serialize_entry("bytes", &self.data.len())?;
        map.serialize_entry("data", &BASE64.encode(&self.data))?;
        map.end()
    }
}

impl fmt::Debug for OutputImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputImage")
            .field("media_type", &self.media_type)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.data.len())
            .finish()
    }
}

/// `Timings`: milliseconds per stage plus `totalMs`. Stage `decode` becomes `decodeMs`.
#[derive(Clone, Debug, PartialEq)]
pub struct Timings {
    pub total: Duration,
    pub stages: Vec<(&'static str, Duration)>,
}

impl Serialize for Timings {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        fn ms(d: Duration) -> f64 {
            (d.as_secs_f64() * 100_000.0).round() / 100.0
        }
        let mut map = serializer.serialize_map(Some(self.stages.len() + 1))?;
        map.serialize_entry("totalMs", &ms(self.total))?;
        for (stage, elapsed) in &self.stages {
            map.serialize_entry(&format!("{stage}Ms"), &ms(*elapsed))?;
        }
        map.end()
    }
}

// --------------------------------------------------------------------- assessment bodies

/// `SubjectAssessment`: verdict, checks, reasons and warnings (from `face_core::evaluate`) plus
/// the primary face.
#[derive(Clone, Debug, Serialize)]
pub struct SubjectAssessment {
    #[serde(flatten)]
    pub assessment: Assessment,
    pub face: Option<FaceInfo>,
}

impl SubjectAssessment {
    pub fn new(assessment: Assessment, face: Option<FaceInfo>) -> Self {
        SubjectAssessment { assessment, face }
    }
}

/// `/v1/quality` result without the server-filled fields.
#[derive(Clone, Debug, Serialize)]
pub struct QualityBody {
    #[serde(flatten)]
    pub subject: SubjectAssessment,
    pub output: Option<OutputImage>,
}

/// Clockwise rotation applied to make the portrait upright.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

impl Rotation {
    pub const fn degrees(self) -> u16 {
        match self {
            Rotation::Deg0 => 0,
            Rotation::Deg90 => 90,
            Rotation::Deg180 => 180,
            Rotation::Deg270 => 270,
        }
    }
}

impl Serialize for Rotation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16(self.degrees())
    }
}

/// `DocumentInfo`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentInfo {
    pub detected: bool,
    pub confidence: f64,
    /// Top-left, top-right, bottom-right, bottom-left, in input-image coordinates.
    pub corners: Option<[PointDto; 4]>,
    pub orientation_degrees: Rotation,
}

impl DocumentInfo {
    /// No document outline found (e.g. a tight portrait crop).
    pub fn not_detected() -> Self {
        DocumentInfo {
            detected: false,
            confidence: 0.0,
            corners: None,
            orientation_degrees: Rotation::Deg0,
        }
    }
}

/// `/v1/document-portraits` result without the server-filled fields.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPortraitBody {
    #[serde(flatten)]
    pub subject: SubjectAssessment,
    pub document: DocumentInfo,
    pub portrait_source_box: Option<BoundingBox>,
    pub output: Option<OutputImage>,
}

// ------------------------------------------------------------------------ verification

/// How the verification reference was finally treated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolvedReferenceKind {
    Document,
    Portrait,
}

/// `VerificationResult.reference`.
#[derive(Clone, Debug, Serialize)]
pub struct ReferenceAssessment {
    #[serde(flatten)]
    pub subject: SubjectAssessment,
    pub kind: ResolvedReferenceKind,
    pub document: Option<DocumentInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatingPointRef {
    pub id: String,
    pub target_fmr: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CalibrationRef {
    pub id: String,
    pub status: CalibrationStatus,
}

/// `VerificationResult.risk`.
#[derive(Clone, Debug, Serialize)]
pub struct RiskSummary {
    /// True when an error-severity risk check failed.
    pub flagged: bool,
    pub checks: Vec<CheckResult>,
}

/// `/v1/verifications` result without the server-filled fields.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationBody {
    pub outcome: Outcome,
    pub decision: Decision,
    /// Cosine similarity. Never logged.
    pub score: Option<f64>,
    pub threshold: f64,
    pub operating_point: OperatingPointRef,
    pub calibration: CalibrationRef,
    pub probe: SubjectAssessment,
    pub reference: ReferenceAssessment,
    pub risk: RiskSummary,
    pub reasons: Vec<Reason>,
}

impl VerificationBody {
    /// Apply the contract's decision and outcome rules ([`face_core::judge`]) to evaluated
    /// assessments and a similarity score, and assemble the body.
    pub fn judge(
        profile: &Profile,
        operating_point: &OperatingPoint,
        probe: SubjectAssessment,
        reference: ReferenceAssessment,
        risk: Assessment,
        score: Option<f64>,
    ) -> Self {
        let score = score.filter(|s| s.is_finite());
        let judgement = face_core::judge(
            Catalogue::embedded(),
            &VerificationInput {
                probe: &probe.assessment,
                reference: &reference.subject.assessment,
                risk: &risk,
                required_probe: &profile.verification.required_probe_checks,
                required_reference: &profile.verification.required_reference_checks,
                score,
                threshold: operating_point.threshold,
            },
        );
        VerificationBody {
            outcome: judgement.outcome,
            decision: judgement.decision,
            score,
            threshold: operating_point.threshold,
            operating_point: OperatingPointRef {
                id: operating_point.id.clone(),
                target_fmr: operating_point.target_fmr,
            },
            calibration: CalibrationRef {
                id: profile.verification.calibration.id.clone(),
                status: profile.verification.calibration.status,
            },
            probe,
            reference,
            risk: RiskSummary {
                flagged: judgement.risk_flagged,
                checks: risk.checks,
            },
            reasons: judgement.reasons,
        }
    }
}

// ----------------------------------------------------------------- server-wrapped results

/// `QualityResult`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityResult<'a> {
    pub request_id: &'a str,
    /// Always `selfie`.
    pub subject: &'static str,
    #[serde(flatten)]
    pub body: &'a QualityBody,
    pub profile: &'a ProfileRef,
    pub engine: &'a EngineInfo,
    pub timings: &'a Timings,
}

/// `DocumentPortraitResult`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPortraitResult<'a> {
    pub request_id: &'a str,
    /// Always `document`.
    pub subject: &'static str,
    #[serde(flatten)]
    pub body: &'a DocumentPortraitBody,
    pub profile: &'a ProfileRef,
    pub engine: &'a EngineInfo,
    pub timings: &'a Timings,
}

/// `VerificationResult`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationResult<'a> {
    pub request_id: &'a str,
    #[serde(flatten)]
    pub body: &'a VerificationBody,
    pub profile: &'a ProfileRef,
    pub engine: &'a EngineInfo,
    pub timings: &'a Timings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_image_serialises_size_and_base64() {
        let out = OutputImage {
            media_type: OutputMediaType::Jpeg,
            width: 2,
            height: 3,
            data: vec![0xFF, 0xD8, 0xFF],
        };
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json["mediaType"], "image/jpeg");
        assert_eq!(json["bytes"], 3);
        assert_eq!(json["data"], "/9j/");
        assert!(!format!("{out:?}").contains("255"));
    }

    #[test]
    fn timings_use_ms_suffixed_keys() {
        let t = Timings {
            total: Duration::from_micros(12_345),
            stages: vec![("decode", Duration::from_micros(1_500))],
        };
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["totalMs"], 12.35);
        assert_eq!(json["decodeMs"], 1.5);
    }

    #[test]
    fn face_info_is_never_debug_printed() {
        let face = FaceInfo {
            bounding_box: BoundingBox {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            },
            landmarks5: [[1.0, 2.0]; 5],
            inter_eye_distance_px: 61.5,
            pose: None,
        };
        assert_eq!(format!("{face:?}"), "FaceInfo { .. }");
        let json = serde_json::to_value(&face).unwrap();
        assert_eq!(json["box"]["width"], 3.0);
        assert_eq!(json["interEyeDistancePx"], 61.5);
        assert!(json.get("pose").is_none());
    }
}
