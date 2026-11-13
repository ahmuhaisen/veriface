//! Profiles: typed form of `contracts/profile.schema.json`, plus the semantic rules the schema
//! cannot express (check ids exist and apply to the section's subject, ranges are sane, required
//! verification checks are enabled, ...).

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::catalogue::{Catalogue, Group, Subject};
use crate::check_id::CheckId;
use crate::evaluate::Severity;

/// Check settings of one profile section, keyed by check id.
pub type CheckSettings = BTreeMap<CheckId, CheckSetting>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub catalogue_version: u32,
    pub input: InputSettings,
    pub face_counting: FaceCounting,
    pub selfie: SubjectSection,
    pub document: SubjectSection,
    pub verification: VerificationSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputSettings {
    pub max_bytes: u64,
    pub max_pixels: u64,
    pub formats: Vec<ImageFormat>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Jpeg,
    Png,
    Webp,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FaceCounting {
    pub min_inter_eye_distance_ratio: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubjectSection {
    pub checks: CheckSettings,
    pub output: OutputSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckSetting {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputSettings {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<OutputStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_jpeg_quality: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Background>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputStyle {
    /// ICAO-proportioned head-and-shoulders crop.
    #[serde(rename = "portrait-3x4")]
    Portrait3x4,
    /// Tight crop around the face, never upscaled.
    FaceCrop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Jpeg,
    Png,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Background {
    Keep,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VerificationSettings {
    pub model: String,
    pub required_probe_checks: Vec<CheckId>,
    pub required_reference_checks: Vec<CheckId>,
    pub operating_points: Vec<OperatingPoint>,
    pub default_operating_point: String,
    pub calibration: Calibration,
    pub risk: RiskSettings,
}

impl VerificationSettings {
    /// The operating point with this id, or the default when `id` is `None`.
    pub fn operating_point(&self, id: Option<&str>) -> Option<&OperatingPoint> {
        let id = id.unwrap_or(&self.default_operating_point);
        self.operating_points.iter().find(|op| op.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperatingPoint {
    pub id: String,
    pub target_fmr: f64,
    pub threshold: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Calibration {
    pub id: String,
    pub status: CalibrationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CalibrationStatus {
    Provisional,
    Calibrated,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RiskSettings {
    pub checks: CheckSettings,
}

/// Why a profile could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("profile YAML is invalid: {0}")]
    Parse(String),
    #[error("profile `{name}` is invalid:\n  - {}", .problems.join("\n  - "))]
    Invalid { name: String, problems: Vec<String> },
}

impl Profile {
    /// Parse a profile and validate it against the catalogue.
    pub fn from_yaml(yaml: &str, catalogue: &Catalogue) -> Result<Profile, ProfileError> {
        let profile: Profile =
            serde_saphyr::from_str(yaml).map_err(|e| ProfileError::Parse(e.to_string()))?;
        profile.validate(catalogue)?;
        Ok(profile)
    }

    /// Semantic validation (everything the JSON schema cannot express). Collects all problems.
    pub fn validate(&self, catalogue: &Catalogue) -> Result<(), ProfileError> {
        let mut problems = Vec::new();

        if !is_valid_name(&self.name) {
            problems.push(format!(
                "name `{}` must match ^[a-z0-9][a-z0-9-]{{1,62}}$",
                self.name
            ));
        }
        if self.catalogue_version != catalogue.version() {
            problems.push(format!(
                "catalogueVersion {} does not match the service catalogue version {}",
                self.catalogue_version,
                catalogue.version()
            ));
        }
        if self.input.formats.is_empty() {
            problems.push("input.formats must not be empty".to_owned());
        }
        if self.input.max_bytes < 1024 || self.input.max_pixels < 10_000 {
            problems.push("input.maxBytes must be >= 1024 and input.maxPixels >= 10000".to_owned());
        }
        if !(0.0..=0.5).contains(&self.face_counting.min_inter_eye_distance_ratio) {
            problems
                .push("faceCounting.minInterEyeDistanceRatio must be within [0, 0.5]".to_owned());
        }

        validate_section(
            catalogue,
            "selfie",
            &self.selfie.checks,
            &[Subject::Selfie],
            &mut problems,
        );
        validate_section(
            catalogue,
            "document",
            &self.document.checks,
            &[Subject::Document, Subject::Portrait],
            &mut problems,
        );
        validate_output("selfie.output", &self.selfie.output, &mut problems);
        validate_output("document.output", &self.document.output, &mut problems);

        let v = &self.verification;
        if v.model.trim().is_empty() {
            problems.push("verification.model must not be empty".to_owned());
        }
        for id in &v.required_probe_checks {
            if !self.selfie.checks.get(id).is_some_and(|s| s.enabled) {
                problems.push(format!(
                    "verification.requiredProbeChecks: {id} must be enabled in selfie.checks"
                ));
            }
        }
        for id in &v.required_reference_checks {
            if !self.document.checks.get(id).is_some_and(|s| s.enabled) {
                problems.push(format!(
                    "verification.requiredReferenceChecks: {id} must be enabled in document.checks"
                ));
            }
        }
        let mut op_ids = HashSet::new();
        for op in &v.operating_points {
            if !op_ids.insert(op.id.as_str()) {
                problems.push(format!(
                    "verification.operatingPoints: duplicate id `{}`",
                    op.id
                ));
            }
            if !(op.target_fmr > 0.0 && op.target_fmr <= 0.1) {
                problems.push(format!(
                    "operating point `{}`: targetFmr must be in (0, 0.1]",
                    op.id
                ));
            }
            if !(-1.0..=1.0).contains(&op.threshold) {
                problems.push(format!(
                    "operating point `{}`: threshold must be in [-1, 1]",
                    op.id
                ));
            }
        }
        if v.operating_points.is_empty() {
            problems.push("verification.operatingPoints must not be empty".to_owned());
        }
        if v.operating_point(None).is_none() {
            problems.push(format!(
                "verification.defaultOperatingPoint `{}` is not one of the operating points",
                v.default_operating_point
            ));
        }
        for (id, setting) in &v.risk.checks {
            if catalogue.get(*id).group != Group::Risk {
                problems.push(format!(
                    "verification.risk.checks: {id} is not a risk check"
                ));
            }
            validate_setting(
                &format!("verification.risk.checks.{id}"),
                setting,
                &mut problems,
            );
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(ProfileError::Invalid {
                name: self.name.clone(),
                problems,
            })
        }
    }
}

fn is_valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (2..=63).contains(&bytes.len())
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn validate_section(
    catalogue: &Catalogue,
    section: &str,
    checks: &CheckSettings,
    subjects: &[Subject],
    problems: &mut Vec<String>,
) {
    for (id, setting) in checks {
        let def = catalogue.get(*id);
        if def.group == Group::Risk {
            problems.push(format!(
                "{section}.checks: {id} is a risk check; configure it under verification.risk.checks"
            ));
        } else if !def.applies_to(subjects) {
            problems.push(format!(
                "{section}.checks: {id} does not apply to this section's subjects"
            ));
        }
        validate_setting(&format!("{section}.checks.{id}"), setting, problems);
    }
}

fn validate_setting(path: &str, setting: &CheckSetting, problems: &mut Vec<String>) {
    if !setting.enabled {
        return;
    }
    match (setting.min, setting.max) {
        (None, None) => problems.push(format!("{path}: an enabled check needs min and/or max")),
        (Some(min), Some(max)) if min > max => {
            problems.push(format!("{path}: min {min} is greater than max {max}"));
        }
        _ => {}
    }
    for bound in [setting.min, setting.max].into_iter().flatten() {
        if !bound.is_finite() {
            problems.push(format!("{path}: bounds must be finite numbers"));
        }
    }
}

fn validate_output(path: &str, output: &OutputSettings, problems: &mut Vec<String>) {
    if !output.enabled {
        return;
    }
    if output.style.is_none()
        || output.width.is_none()
        || output.height.is_none()
        || output.format.is_none()
    {
        problems.push(format!(
            "{path}: enabled output needs style, width, height and format"
        ));
    }
    for (name, v) in [("width", output.width), ("height", output.height)] {
        if let Some(v) = v
            && !(64..=4096).contains(&v)
        {
            problems.push(format!("{path}.{name} must be within [64, 4096]"));
        }
    }
    if output.format == Some(OutputFormat::Jpeg) {
        match (output.max_bytes, output.min_jpeg_quality) {
            (Some(bytes), Some(q)) => {
                if bytes < 4096 {
                    problems.push(format!("{path}.maxBytes must be >= 4096"));
                }
                if !(30..=100).contains(&q) {
                    problems.push(format!("{path}.minJpegQuality must be within [30, 100]"));
                }
            }
            _ => problems.push(format!(
                "{path}: JPEG output needs maxBytes and minJpegQuality"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANDARD: &str = include_str!("../../../../profiles/standard.yaml");
    const ICAO_STRICT: &str = include_str!("../../../../profiles/icao-strict.yaml");

    #[test]
    fn shipped_profiles_are_valid() {
        let cat = Catalogue::embedded();
        for yaml in [STANDARD, ICAO_STRICT] {
            if let Err(e) = Profile::from_yaml(yaml, cat) {
                panic!("{e}");
            }
        }
    }

    #[test]
    fn standard_profile_has_expected_shape() {
        let p = Profile::from_yaml(STANDARD, Catalogue::embedded()).unwrap();
        assert_eq!(p.name, "standard");
        assert_eq!(p.selfie.output.style, Some(OutputStyle::Portrait3x4));
        assert_eq!(p.verification.operating_point(None).unwrap().id, "fmr-1e-4");
        assert_eq!(
            p.verification.calibration.status,
            CalibrationStatus::Provisional
        );
        assert!(p.selfie.checks[&CheckId::HeadPoseYaw].enabled);
    }

    #[test]
    fn rejects_unknown_fields_and_unknown_check_ids() {
        let cat = Catalogue::embedded();
        let typo = STANDARD.replace("maxPixels", "maxPixel");
        assert!(matches!(
            Profile::from_yaml(&typo, cat),
            Err(ProfileError::Parse(_))
        ));
        let unknown = STANDARD.replace("HEAD_POSE_YAW", "HEAD_POSE_YAWN");
        assert!(matches!(
            Profile::from_yaml(&unknown, cat),
            Err(ProfileError::Parse(_))
        ));
    }

    #[test]
    fn reports_semantic_problems() {
        let cat = Catalogue::embedded();
        let mut p = Profile::from_yaml(STANDARD, cat).unwrap();
        // A risk check in the selfie section, an inverted range, a missing default operating
        // point and a required check that is disabled.
        p.selfie.checks.insert(
            CheckId::ProbeScreenArtifacts,
            CheckSetting {
                enabled: true,
                severity: None,
                min: None,
                max: Some(0.5),
            },
        );
        p.selfie.checks.insert(
            CheckId::HeadPoseRoll,
            CheckSetting {
                enabled: true,
                severity: None,
                min: Some(10.0),
                max: Some(-10.0),
            },
        );
        p.verification.default_operating_point = "nope".to_owned();
        p.selfie
            .checks
            .get_mut(&CheckId::Sharpness)
            .unwrap()
            .enabled = false;
        let Err(ProfileError::Invalid { problems, .. }) = p.validate(cat) else {
            panic!("expected validation problems");
        };
        assert_eq!(problems.len(), 4, "{problems:#?}");
    }
}
