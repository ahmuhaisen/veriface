//! The seam between the HTTP layer and the vision pipeline.
//!
//! The server owns HTTP, auth, limits, decoding, timings and metrics. An [`Engine`] receives
//! decoded images plus the selected profile and returns the OpenAPI result bodies without the
//! fields the server fills in (`requestId`, `profile`, `engine`, `timings`).
//!
//! Engines are called on a blocking thread, at most `server.max_concurrency` at a time. They
//! build their bodies with [`face_core::evaluate`] (assessments) and
//! [`VerificationBody::judge`] (verification decision and outcome), so the contract logic stays
//! in `face-core`.

use std::time::{Duration, Instant};

use face_core::{
    Catalogue, CheckSettings, DOCUMENT_ASSESSMENT, Measurements, OperatingPoint, Profile,
    RISK_ASSESSMENT, SELFIE_ASSESSMENT, Subject, evaluate,
};
use face_imaging::codec::Decoded;
use serde::Deserialize;

use crate::dto::{
    DocumentInfo, DocumentPortraitBody, EngineInfo, QualityBody, ReferenceAssessment,
    ResolvedReferenceKind, SubjectAssessment, VerificationBody,
};

/// `referenceKind` query parameter of `/v1/verifications`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceKind {
    /// Try document extraction; fall back to treating the image as a portrait.
    #[default]
    Auto,
    /// The reference is a document image; the portrait is extracted.
    Document,
    /// The reference is already a face portrait (rectification is skipped).
    Portrait,
}

/// Input of [`Engine::assess_quality`].
#[derive(Clone, Copy)]
pub struct QualityRequest<'a> {
    pub profile: &'a Profile,
    pub image: &'a Decoded,
    /// Produce `output` (query `output`, defaulting to `profile.selfie.output.enabled`). The
    /// server has checked that the profile's output settings are complete when this is true.
    pub include_output: bool,
}

/// Input of [`Engine::extract_document_portrait`].
#[derive(Clone, Copy)]
pub struct DocumentPortraitRequest<'a> {
    pub profile: &'a Profile,
    pub document: &'a Decoded,
    /// As for [`QualityRequest::include_output`], with `profile.document.output`.
    pub include_output: bool,
}

/// Input of [`Engine::verify`].
#[derive(Clone, Copy)]
pub struct VerificationRequest<'a> {
    pub profile: &'a Profile,
    /// Already resolved (query `operatingPoint`, else the profile default).
    pub operating_point: &'a OperatingPoint,
    pub reference_kind: ReferenceKind,
    pub probe: &'a Decoded,
    pub reference: &'a Decoded,
}

/// Why an engine call failed. The message goes to logs only, never to the client.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The engine cannot serve requests (e.g. a model session failed): `503 not-ready`.
    #[error("engine unavailable: {0}")]
    Unavailable(String),
    /// Anything else: `500 internal`.
    #[error("engine failure: {0}")]
    Internal(String),
}

/// Stage timings recorded during a request. Stage names are short lowerCamel identifiers (they
/// become `<stage>Ms` in `timings` and the `stage` label of `face_stage_duration_seconds`), so
/// keep the set small and fixed.
#[derive(Clone, Debug, Default)]
pub struct Stages {
    entries: Vec<(&'static str, Duration)>,
}

impl Stages {
    /// Add `elapsed` to `stage` (repeated stages accumulate).
    pub fn record(&mut self, stage: &'static str, elapsed: Duration) {
        match self.entries.iter_mut().find(|(name, _)| *name == stage) {
            Some((_, total)) => *total += elapsed,
            None => self.entries.push((stage, elapsed)),
        }
    }

    /// Run `f` and record how long it took.
    pub fn time<T>(&mut self, stage: &'static str, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = f();
        self.record(stage, start.elapsed());
        out
    }

    pub fn entries(&self) -> &[(&'static str, Duration)] {
        &self.entries
    }
}

/// The vision pipeline, as seen by the server.
pub trait Engine: Send + Sync + 'static {
    /// Service, catalogue and model versions (`/v1/info` and `engine` in every result).
    fn info(&self) -> &EngineInfo;

    /// Load and verify models, create sessions and run a warm-up inference. Called once, on a
    /// blocking thread, after the listener is bound. `/health/ready` reports 200 only after it
    /// returns `Ok`; an error stops the service.
    fn warm_up(&self) -> Result<(), EngineError>;

    /// Startup check of a loaded profile against what the engine can run (e.g. that
    /// `verification.model` is the loaded embedder). An error stops the service.
    fn check_profile(&self, profile: &Profile) -> Result<(), String> {
        let _ = profile;
        Ok(())
    }

    /// `/v1/quality`: assess a selfie.
    fn assess_quality(
        &self,
        request: &QualityRequest<'_>,
        stages: &mut Stages,
    ) -> Result<QualityBody, EngineError>;

    /// `/v1/document-portraits`: find and assess the portrait on a document.
    fn extract_document_portrait(
        &self,
        request: &DocumentPortraitRequest<'_>,
        stages: &mut Stages,
    ) -> Result<DocumentPortraitBody, EngineError>;

    /// `/v1/verifications`: compare a selfie with a document portrait.
    fn verify(
        &self,
        request: &VerificationRequest<'_>,
        stages: &mut Stages,
    ) -> Result<VerificationBody, EngineError>;
}

/// **STUB ENGINE: no vision pipeline.** Every enabled check is `undetermined`, verdicts are
/// `undetermined` and the verification outcome is `review` (no score). It exists for tests and
/// for running the service before the vision crates are wired in. Never deploy it: the service
/// logs a warning at startup and reports `+stub` in `engine.version`.
#[derive(Debug)]
pub struct StubEngine {
    info: EngineInfo,
}

impl StubEngine {
    pub const CAUSE: &'static str = "stub engine: no vision pipeline";

    pub fn new() -> Self {
        StubEngine {
            info: EngineInfo {
                version: format!("{}+stub", env!("CARGO_PKG_VERSION")),
                catalogue_version: Catalogue::embedded().version(),
                models: Vec::new(),
            },
        }
    }

    fn assess(subjects: &[Subject], settings: &CheckSettings) -> face_core::Assessment {
        let mut measurements = Measurements::new();
        for id in settings.keys() {
            measurements.set_undetermined(*id, Self::CAUSE);
        }
        evaluate(Catalogue::embedded(), subjects, settings, &measurements)
    }
}

impl Default for StubEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for StubEngine {
    fn info(&self) -> &EngineInfo {
        &self.info
    }

    fn warm_up(&self) -> Result<(), EngineError> {
        Ok(())
    }

    fn assess_quality(
        &self,
        request: &QualityRequest<'_>,
        stages: &mut Stages,
    ) -> Result<QualityBody, EngineError> {
        let assessment = stages.time("evaluate", || {
            Self::assess(SELFIE_ASSESSMENT, &request.profile.selfie.checks)
        });
        Ok(QualityBody {
            subject: SubjectAssessment::new(assessment, None),
            output: None,
        })
    }

    fn extract_document_portrait(
        &self,
        request: &DocumentPortraitRequest<'_>,
        stages: &mut Stages,
    ) -> Result<DocumentPortraitBody, EngineError> {
        let assessment = stages.time("evaluate", || {
            Self::assess(DOCUMENT_ASSESSMENT, &request.profile.document.checks)
        });
        Ok(DocumentPortraitBody {
            subject: SubjectAssessment::new(assessment, None),
            document: DocumentInfo::not_detected(),
            portrait_source_box: None,
            output: None,
        })
    }

    fn verify(
        &self,
        request: &VerificationRequest<'_>,
        stages: &mut Stages,
    ) -> Result<VerificationBody, EngineError> {
        let profile = request.profile;
        let (probe, reference, risk) = stages.time("evaluate", || {
            (
                Self::assess(SELFIE_ASSESSMENT, &profile.selfie.checks),
                Self::assess(DOCUMENT_ASSESSMENT, &profile.document.checks),
                Self::assess(RISK_ASSESSMENT, &profile.verification.risk.checks),
            )
        });
        // Nothing is detected, so `auto` falls back to a portrait reference.
        let (kind, document) = match request.reference_kind {
            ReferenceKind::Document => (
                ResolvedReferenceKind::Document,
                Some(DocumentInfo::not_detected()),
            ),
            ReferenceKind::Auto | ReferenceKind::Portrait => {
                (ResolvedReferenceKind::Portrait, None)
            }
        };
        Ok(VerificationBody::judge(
            profile,
            request.operating_point,
            SubjectAssessment::new(probe, None),
            ReferenceAssessment {
                subject: SubjectAssessment::new(reference, None),
                kind,
                document,
            },
            risk,
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_accumulate_repeated_names() {
        let mut stages = Stages::default();
        stages.record("detect", Duration::from_millis(2));
        stages.record("decode", Duration::from_millis(1));
        stages.record("detect", Duration::from_millis(3));
        assert_eq!(
            stages.entries(),
            &[
                ("detect", Duration::from_millis(5)),
                ("decode", Duration::from_millis(1))
            ]
        );
    }
}
