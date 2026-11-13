//! Measurements -> statuses -> verdict, exactly as specified in `contracts/checks.yaml`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::catalogue::{Catalogue, Group, Subject};
use crate::check_id::CheckId;
use crate::profile::CheckSettings;

/// Check status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Fail,
    Undetermined,
    Skipped,
}

/// Check severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// Quality-style verdict for one assessment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Accepted,
    Rejected,
    Undetermined,
}

/// What a vision crate reports for one check.
#[derive(Clone, Debug, PartialEq)]
pub enum Measurement {
    /// The measured value, in the unit the catalogue defines.
    Value(f64),
    /// The check could not be assessed (e.g. the estimator was not confident). The cause goes
    /// to logs only, never to the API.
    Undetermined(String),
}

/// Measurements for one assessment, keyed by check id.
#[derive(Clone, Debug, Default)]
pub struct Measurements(HashMap<CheckId, Measurement>);

impl Measurements {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_value(&mut self, id: CheckId, value: f64) {
        self.0.insert(id, Measurement::Value(value));
    }

    pub fn set_undetermined(&mut self, id: CheckId, cause: impl Into<String>) {
        self.0.insert(id, Measurement::Undetermined(cause.into()));
    }

    pub fn get(&self, id: CheckId) -> Option<&Measurement> {
        self.0.get(&id)
    }

    /// The numeric value, if one was measured.
    pub fn value(&self, id: CheckId) -> Option<f64> {
        match self.0.get(&id) {
            Some(Measurement::Value(v)) => Some(*v),
            _ => None,
        }
    }

    /// Add all of `other`; its entries win on conflict.
    pub fn extend(&mut self, other: Measurements) {
        self.0.extend(other.0);
    }
}

/// Result of one check, shaped like `CheckResult` in `contracts/openapi.yaml`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub id: CheckId,
    pub group: Group,
    pub status: Status,
    pub severity: Severity,
    pub value: Option<f64>,
    pub unit: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub experimental: bool,
    /// Why the check is undetermined. Logs only; not part of the API.
    #[serde(skip)]
    pub cause: Option<String>,
}

/// Evaluated checks plus verdict and ordered reasons/warnings for one assessment.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    pub verdict: Verdict,
    pub checks: Vec<CheckResult>,
    /// Failed error-severity checks, root causes first.
    pub reasons: Vec<CheckId>,
    /// Failed warning-severity checks, root causes first.
    pub warnings: Vec<CheckId>,
}

impl Assessment {
    pub fn result(&self, id: CheckId) -> Option<&CheckResult> {
        self.checks.iter().find(|c| c.id == id)
    }

    pub fn status(&self, id: CheckId) -> Option<Status> {
        self.result(id).map(|c| c.status)
    }
}

/// Evaluate the configured checks of one assessment.
///
/// - `subjects`: the assessment's subjects (e.g. [`crate::SELFIE_ASSESSMENT`]).
/// - `settings`: the profile section's check settings. Checks not listed are omitted from the
///   result; checks listed with `enabled: false` are reported as `skipped`.
/// - `measurements`: what the pipeline measured. An enabled check without a measurement is
///   `undetermined` ("not measured").
pub fn evaluate(
    catalogue: &Catalogue,
    subjects: &[Subject],
    settings: &CheckSettings,
    measurements: &Measurements,
) -> Assessment {
    let mut memo: HashMap<CheckId, CheckResult> = HashMap::new();
    for def in catalogue.checks() {
        if settings.contains_key(&def.id) && def.applies_to(subjects) {
            resolve(
                catalogue,
                subjects,
                settings,
                measurements,
                def.id,
                &mut memo,
            );
        }
    }

    // Catalogue order for `checks`.
    let checks: Vec<CheckResult> = catalogue
        .checks()
        .iter()
        .filter_map(|def| memo.remove(&def.id))
        .collect();

    let mut reasons: Vec<CheckId> = checks
        .iter()
        .filter(|c| c.status == Status::Fail && c.severity == Severity::Error)
        .map(|c| c.id)
        .collect();
    let mut warnings: Vec<CheckId> = checks
        .iter()
        .filter(|c| c.status == Status::Fail && c.severity == Severity::Warning)
        .map(|c| c.id)
        .collect();
    reasons.sort_by_key(|id| catalogue.reason_rank(*id));
    warnings.sort_by_key(|id| catalogue.reason_rank(*id));

    let verdict = if !reasons.is_empty() {
        Verdict::Rejected
    } else if checks
        .iter()
        .any(|c| c.severity == Severity::Error && c.status == Status::Undetermined)
    {
        Verdict::Undetermined
    } else {
        Verdict::Accepted
    };

    Assessment {
        verdict,
        checks,
        reasons,
        warnings,
    }
}

/// Resolve one check (and, recursively, its prerequisites). Returns its status, or `None` when
/// the check is not part of this assessment (not configured or not applicable), in which case
/// it is ignored as a prerequisite.
fn resolve(
    catalogue: &Catalogue,
    subjects: &[Subject],
    settings: &CheckSettings,
    measurements: &Measurements,
    id: CheckId,
    memo: &mut HashMap<CheckId, CheckResult>,
) -> Option<Status> {
    if let Some(done) = memo.get(&id) {
        return Some(done.status);
    }
    let def = catalogue.get(id);
    let setting = settings.get(&id)?;
    if !def.applies_to(subjects) {
        return None;
    }

    let severity = setting.severity.unwrap_or(def.default_severity);
    let base = CheckResult {
        id,
        group: def.group,
        status: Status::Skipped,
        severity,
        value: None,
        unit: def.unit.clone(),
        min: setting.min,
        max: setting.max,
        experimental: def.experimental,
        cause: None,
    };

    if !setting.enabled {
        memo.insert(id, base);
        return Some(Status::Skipped);
    }

    // Prerequisites: a fail or undetermined prerequisite makes this check undetermined.
    let mut blocking = None;
    for p in &def.prerequisites {
        let prerequisite = resolve(catalogue, subjects, settings, measurements, *p, memo);
        if matches!(prerequisite, Some(Status::Fail | Status::Undetermined)) {
            blocking = Some(*p);
            break;
        }
    }

    let result = if let Some(p) = blocking {
        CheckResult {
            status: Status::Undetermined,
            cause: Some(format!("prerequisite {p} did not pass")),
            ..base
        }
    } else {
        match measurements.get(id) {
            None => CheckResult {
                status: Status::Undetermined,
                cause: Some("not measured".to_owned()),
                ..base
            },
            Some(Measurement::Undetermined(cause)) => CheckResult {
                status: Status::Undetermined,
                cause: Some(cause.clone()),
                ..base
            },
            Some(Measurement::Value(v)) if !v.is_finite() => CheckResult {
                status: Status::Undetermined,
                cause: Some("non-finite value".to_owned()),
                ..base
            },
            Some(Measurement::Value(v)) => {
                let within = setting.min.is_none_or(|min| *v >= min)
                    && setting.max.is_none_or(|max| *v <= max);
                CheckResult {
                    status: if within { Status::Pass } else { Status::Fail },
                    value: Some(*v),
                    ..base
                }
            }
        }
    };

    let status = result.status;
    memo.insert(id, result);
    Some(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::CheckSetting;
    use crate::{DOCUMENT_ASSESSMENT, SELFIE_ASSESSMENT};

    fn setting(min: Option<f64>, max: Option<f64>) -> CheckSetting {
        CheckSetting {
            enabled: true,
            severity: None,
            min,
            max,
        }
    }

    fn selfie_settings() -> CheckSettings {
        let mut s = CheckSettings::new();
        s.insert(CheckId::FacePresent, setting(Some(1.0), None));
        s.insert(CheckId::SingleFace, setting(None, Some(1.0)));
        s.insert(CheckId::InterEyeDistance, setting(Some(60.0), None));
        s.insert(CheckId::HeadPoseYaw, setting(Some(-15.0), Some(15.0)));
        s.insert(
            CheckId::ExpressionNeutral,
            CheckSetting {
                severity: Some(Severity::Warning),
                ..setting(None, Some(0.6))
            },
        );
        s.insert(
            CheckId::Eyeglasses,
            CheckSetting {
                enabled: false,
                severity: None,
                min: None,
                max: Some(0.5),
            },
        );
        s
    }

    fn good_selfie() -> Measurements {
        let mut m = Measurements::new();
        m.set_value(CheckId::FacePresent, 1.0);
        m.set_value(CheckId::SingleFace, 1.0);
        m.set_value(CheckId::InterEyeDistance, 120.0);
        m.set_value(CheckId::HeadPoseYaw, 3.0);
        m.set_value(CheckId::ExpressionNeutral, 0.1);
        m
    }

    #[test]
    fn accepts_a_good_selfie_and_reports_skipped_checks() {
        let a = evaluate(
            Catalogue::embedded(),
            SELFIE_ASSESSMENT,
            &selfie_settings(),
            &good_selfie(),
        );
        assert_eq!(a.verdict, Verdict::Accepted);
        assert_eq!(a.status(CheckId::Eyeglasses), Some(Status::Skipped));
        assert!(a.reasons.is_empty() && a.warnings.is_empty());
        // Checks come out in catalogue order.
        let ids: Vec<CheckId> = a.checks.iter().map(|c| c.id).collect();
        assert_eq!(ids.first(), Some(&CheckId::FacePresent));
    }

    #[test]
    fn range_bounds_are_inclusive() {
        let mut m = good_selfie();
        m.set_value(CheckId::HeadPoseYaw, 15.0);
        let a = evaluate(
            Catalogue::embedded(),
            SELFIE_ASSESSMENT,
            &selfie_settings(),
            &m,
        );
        assert_eq!(a.status(CheckId::HeadPoseYaw), Some(Status::Pass));
    }

    #[test]
    fn failed_prerequisite_makes_dependents_undetermined_and_only_root_cause_is_a_reason() {
        let mut m = good_selfie();
        m.set_value(CheckId::FacePresent, 0.0);
        let a = evaluate(
            Catalogue::embedded(),
            SELFIE_ASSESSMENT,
            &selfie_settings(),
            &m,
        );
        assert_eq!(a.verdict, Verdict::Rejected);
        assert_eq!(a.reasons, vec![CheckId::FacePresent]);
        assert_eq!(a.status(CheckId::SingleFace), Some(Status::Undetermined));
        assert_eq!(a.status(CheckId::HeadPoseYaw), Some(Status::Undetermined));
        assert_eq!(a.result(CheckId::HeadPoseYaw).unwrap().value, None);
    }

    #[test]
    fn warnings_never_change_the_verdict() {
        let mut m = good_selfie();
        m.set_value(CheckId::ExpressionNeutral, 0.9);
        let a = evaluate(
            Catalogue::embedded(),
            SELFIE_ASSESSMENT,
            &selfie_settings(),
            &m,
        );
        assert_eq!(a.verdict, Verdict::Accepted);
        assert_eq!(a.warnings, vec![CheckId::ExpressionNeutral]);
    }

    #[test]
    fn undetermined_error_check_gives_undetermined_verdict() {
        let mut m = good_selfie();
        m.set_undetermined(CheckId::HeadPoseYaw, "low landmark confidence");
        let a = evaluate(
            Catalogue::embedded(),
            SELFIE_ASSESSMENT,
            &selfie_settings(),
            &m,
        );
        assert_eq!(a.verdict, Verdict::Undetermined);
        assert_eq!(
            a.result(CheckId::HeadPoseYaw).unwrap().cause.as_deref(),
            Some("low landmark confidence")
        );
    }

    #[test]
    fn missing_and_non_finite_measurements_are_undetermined() {
        let mut m = good_selfie();
        m.set_value(CheckId::HeadPoseYaw, f64::NAN);
        let mut s = selfie_settings();
        s.insert(CheckId::Sharpness, setting(Some(0.3), None));
        let a = evaluate(Catalogue::embedded(), SELFIE_ASSESSMENT, &s, &m);
        assert_eq!(a.status(CheckId::HeadPoseYaw), Some(Status::Undetermined));
        assert_eq!(a.status(CheckId::Sharpness), Some(Status::Undetermined));
        assert_eq!(a.verdict, Verdict::Undetermined);
    }

    #[test]
    fn reasons_follow_catalogue_priority_not_insertion_order() {
        let mut m = good_selfie();
        m.set_value(CheckId::HeadPoseYaw, 40.0);
        m.set_value(CheckId::InterEyeDistance, 20.0);
        let a = evaluate(
            Catalogue::embedded(),
            SELFIE_ASSESSMENT,
            &selfie_settings(),
            &m,
        );
        assert_eq!(
            a.reasons,
            vec![CheckId::InterEyeDistance, CheckId::HeadPoseYaw]
        );
    }

    #[test]
    fn document_assessment_uses_portrait_found_as_prerequisite_and_ignores_selfie_only_ones() {
        let mut s = CheckSettings::new();
        s.insert(CheckId::PortraitFound, setting(Some(1.0), None));
        s.insert(CheckId::InterEyeDistance, setting(Some(60.0), None));
        // FACE_PRESENT is selfie-only: configuring it here must not affect the document assessment.
        s.insert(CheckId::FacePresent, setting(Some(1.0), None));

        let mut m = Measurements::new();
        m.set_value(CheckId::PortraitFound, 0.0);
        m.set_value(CheckId::InterEyeDistance, 80.0);
        let a = evaluate(Catalogue::embedded(), DOCUMENT_ASSESSMENT, &s, &m);
        assert_eq!(a.status(CheckId::FacePresent), None);
        assert_eq!(
            a.status(CheckId::InterEyeDistance),
            Some(Status::Undetermined)
        );
        assert_eq!(a.reasons, vec![CheckId::PortraitFound]);

        m.set_value(CheckId::PortraitFound, 1.0);
        let a = evaluate(Catalogue::embedded(), DOCUMENT_ASSESSMENT, &s, &m);
        assert_eq!(a.verdict, Verdict::Accepted);
    }

    #[test]
    fn skipped_prerequisite_does_not_block() {
        let mut s = selfie_settings();
        s.insert(
            CheckId::SingleFace,
            CheckSetting {
                enabled: false,
                severity: None,
                min: None,
                max: Some(1.0),
            },
        );
        let a = evaluate(Catalogue::embedded(), SELFIE_ASSESSMENT, &s, &good_selfie());
        assert_eq!(a.status(CheckId::HeadPoseYaw), Some(Status::Pass));
    }

    #[test]
    fn serialises_like_the_openapi_check_result() {
        let a = evaluate(
            Catalogue::embedded(),
            SELFIE_ASSESSMENT,
            &selfie_settings(),
            &good_selfie(),
        );
        let json = serde_json::to_value(&a).unwrap();
        let first = &json["checks"][0];
        assert_eq!(first["id"], "FACE_PRESENT");
        assert_eq!(first["group"], "face");
        assert_eq!(first["status"], "pass");
        assert_eq!(first["severity"], "error");
        assert!(first.get("cause").is_none());
        assert_eq!(json["verdict"], "accepted");
    }
}
