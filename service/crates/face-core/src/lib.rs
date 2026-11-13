//! Contract logic for Veriface.
//!
//! This crate is the single place where `contracts/checks.yaml` and `contracts/profile.schema.json`
//! become code:
//! - [`CheckId`] and the [`Catalogue`] (embedded from `contracts/checks.yaml`)
//! - [`Profile`] loading and semantic validation
//! - [`evaluate`]: turns measurements into statuses, a verdict, and ordered reasons/warnings
//! - [`judge`]: verification decision, outcome and reasons
//!
//! Vision crates only produce a [`Measurement`] per check; they never decide statuses.

mod catalogue;
mod check_id;
mod evaluate;
mod profile;
mod verify;

pub use catalogue::{Catalogue, CatalogueError, CheckDef, Group, Subject};
pub use check_id::{CheckId, UnknownCheckId};
pub use evaluate::{
    Assessment, CheckResult, Measurement, Measurements, Severity, Status, Verdict, evaluate,
};
pub use profile::{
    Background, Calibration, CalibrationStatus, CheckSetting, CheckSettings, FaceCounting,
    ImageFormat, InputSettings, OperatingPoint, OutputFormat, OutputSettings, OutputStyle, Profile,
    ProfileError, RiskSettings, SubjectSection, VerificationSettings,
};
pub use verify::{
    Decision, NO_SCORE, Outcome, Reason, ReasonSource, SCORE_BELOW_THRESHOLD, VerificationInput,
    VerificationJudgement, judge,
};

/// Subjects assessed by `/v1/quality` and by the verification probe.
pub const SELFIE_ASSESSMENT: &[Subject] = &[Subject::Selfie];
/// Subjects assessed by `/v1/document-portraits` and by the verification reference.
pub const DOCUMENT_ASSESSMENT: &[Subject] = &[Subject::Document, Subject::Portrait];
/// Subjects assessed by the verification risk checks.
pub const RISK_ASSESSMENT: &[Subject] = &[Subject::Pair, Subject::Selfie];
