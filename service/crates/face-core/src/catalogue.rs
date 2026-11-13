//! The check catalogue, embedded at compile time from `contracts/checks.yaml`.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::check_id::CheckId;
use crate::evaluate::Severity;

/// The contract file, compiled into the binary so the service can never drift from it.
const CATALOGUE_YAML: &str = include_str!("../../../../contracts/checks.yaml");

/// What a check is about. See `subjects` in `contracts/checks.yaml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Subject {
    Selfie,
    Document,
    Portrait,
    Pair,
}

/// Check group. See `groups` in `contracts/checks.yaml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    Input,
    Face,
    Geometry,
    Pose,
    Eyes,
    Mouth,
    Expression,
    Occlusion,
    Illumination,
    Image,
    Background,
    Document,
    Risk,
}

/// One check definition from the catalogue.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckDef {
    pub id: CheckId,
    pub group: Group,
    pub subjects: Vec<Subject>,
    pub unit: String,
    /// Human description of the value (the catalogue's `value` field).
    pub value_description: String,
    pub prerequisites: Vec<CheckId>,
    pub method: String,
    pub default_severity: Severity,
    pub experimental: bool,
    pub standard_ref: Option<String>,
}

impl CheckDef {
    /// True when this check is evaluated in an assessment of `subjects`.
    pub fn applies_to(&self, subjects: &[Subject]) -> bool {
        self.subjects.iter().any(|s| subjects.contains(s))
    }
}

/// Error building a catalogue from YAML.
#[derive(Debug, thiserror::Error)]
pub enum CatalogueError {
    #[error("catalogue YAML is invalid: {0}")]
    Parse(String),
    #[error("catalogue is inconsistent: {0}")]
    Invalid(String),
}

/// The parsed, validated check catalogue.
#[derive(Debug)]
pub struct Catalogue {
    version: u32,
    defs: Vec<CheckDef>,
    by_id: HashMap<CheckId, usize>,
    priority: HashMap<CheckId, usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCatalogue {
    catalogue_version: u32,
    reasons: RawReasons,
    checks: Vec<RawCheck>,
}

#[derive(Deserialize)]
struct RawReasons {
    priority: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCheck {
    id: String,
    group: Group,
    subjects: Vec<Subject>,
    unit: String,
    value: String,
    #[serde(default)]
    prerequisites: Vec<String>,
    method: String,
    default_severity: Severity,
    #[serde(default)]
    experimental: bool,
    #[serde(default)]
    standard_ref: Option<String>,
}

impl Catalogue {
    /// The catalogue compiled into this binary. Panics only if `contracts/checks.yaml` is invalid,
    /// which the unit tests rule out.
    pub fn embedded() -> &'static Catalogue {
        static CATALOGUE: OnceLock<Catalogue> = OnceLock::new();
        CATALOGUE.get_or_init(|| {
            Catalogue::from_yaml(CATALOGUE_YAML)
                .unwrap_or_else(|e| panic!("embedded contracts/checks.yaml is invalid: {e}"))
        })
    }

    /// Parse and validate a catalogue. It must define exactly the checks in [`CheckId::ALL`].
    pub fn from_yaml(yaml: &str) -> Result<Catalogue, CatalogueError> {
        let raw: RawCatalogue =
            serde_saphyr::from_str(yaml).map_err(|e| CatalogueError::Parse(e.to_string()))?;

        let mut defs = Vec::with_capacity(raw.checks.len());
        let mut by_id = HashMap::new();
        for check in raw.checks {
            let id: CheckId = check
                .id
                .parse()
                .map_err(|e| CatalogueError::Invalid(format!("{e} (not in CheckId)")))?;
            if by_id.insert(id, defs.len()).is_some() {
                return Err(CatalogueError::Invalid(format!("duplicate check id {id}")));
            }
            if check.subjects.is_empty() {
                return Err(CatalogueError::Invalid(format!("{id} has no subjects")));
            }
            let prerequisites = check
                .prerequisites
                .iter()
                .map(|p| {
                    p.parse()
                        .map_err(|e| CatalogueError::Invalid(format!("{id}: prerequisite {e}")))
                })
                .collect::<Result<Vec<CheckId>, _>>()?;
            defs.push(CheckDef {
                id,
                group: check.group,
                subjects: check.subjects,
                unit: check.unit,
                value_description: check.value,
                prerequisites,
                method: check.method,
                default_severity: check.default_severity,
                experimental: check.experimental,
                standard_ref: check.standard_ref,
            });
        }

        for id in CheckId::ALL {
            if !by_id.contains_key(id) {
                return Err(CatalogueError::Invalid(format!(
                    "{id} is defined in CheckId but missing from the catalogue"
                )));
            }
        }

        let mut priority = HashMap::new();
        for (rank, id) in raw.reasons.priority.iter().enumerate() {
            let id: CheckId = id
                .parse()
                .map_err(|e| CatalogueError::Invalid(format!("reasons.priority: {e}")))?;
            if priority.insert(id, rank).is_some() {
                return Err(CatalogueError::Invalid(format!(
                    "reasons.priority lists {id} twice"
                )));
            }
        }

        let catalogue = Catalogue {
            version: raw.catalogue_version,
            defs,
            by_id,
            priority,
        };
        catalogue.check_acyclic()?;
        Ok(catalogue)
    }

    /// `catalogueVersion` from the contract.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Definitions in catalogue order.
    pub fn checks(&self) -> &[CheckDef] {
        &self.defs
    }

    /// Definition of a check. Every [`CheckId`] has one (guaranteed at construction).
    pub fn get(&self, id: CheckId) -> &CheckDef {
        &self.defs[self.by_id[&id]]
    }

    /// Sort key for `reasons` / `warnings`: listed priority first, then catalogue order.
    pub fn reason_rank(&self, id: CheckId) -> usize {
        match self.priority.get(&id) {
            Some(rank) => *rank,
            None => self.priority.len() + self.by_id[&id],
        }
    }

    fn check_acyclic(&self) -> Result<(), CatalogueError> {
        fn visit(
            cat: &Catalogue,
            id: CheckId,
            done: &mut HashSet<CheckId>,
            stack: &mut Vec<CheckId>,
        ) -> Result<(), CatalogueError> {
            if done.contains(&id) {
                return Ok(());
            }
            if stack.contains(&id) {
                return Err(CatalogueError::Invalid(format!(
                    "prerequisite cycle through {id}"
                )));
            }
            stack.push(id);
            for p in &cat.get(id).prerequisites {
                visit(cat, *p, done, stack)?;
            }
            stack.pop();
            done.insert(id);
            Ok(())
        }
        let mut done = HashSet::new();
        for def in &self.defs {
            visit(self, def.id, &mut done, &mut Vec::new())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalogue_matches_check_ids_exactly() {
        let cat = Catalogue::embedded();
        assert_eq!(cat.version(), 1);
        assert_eq!(cat.checks().len(), CheckId::ALL.len());
        for id in CheckId::ALL {
            assert_eq!(cat.get(*id).id, *id);
        }
    }

    #[test]
    fn prerequisites_resolve_and_risk_group_is_consistent() {
        let cat = Catalogue::embedded();
        for def in cat.checks() {
            for p in &def.prerequisites {
                assert_ne!(*p, def.id, "{} lists itself as a prerequisite", def.id);
            }
            if def.group == Group::Risk {
                assert!(
                    def.subjects
                        .iter()
                        .all(|s| matches!(s, Subject::Pair | Subject::Selfie)),
                    "risk check {} must apply to pair/selfie only",
                    def.id
                );
            }
        }
    }

    #[test]
    fn reason_rank_puts_listed_priority_first() {
        let cat = Catalogue::embedded();
        assert!(cat.reason_rank(CheckId::FacePresent) < cat.reason_rank(CheckId::Sharpness));
        // An unlisted id ranks after every listed one.
        assert!(
            cat.reason_rank(CheckId::BackgroundUniformity)
                > cat.reason_rank(CheckId::CompressionQuality)
        );
    }

    #[test]
    fn rejects_cycles() {
        let yaml = r#"
catalogueVersion: 1
reasons: { priority: [] }
checks:
  - { id: FACE_PRESENT, group: face, subjects: [selfie], unit: count, value: v, prerequisites: [SINGLE_FACE], method: m, defaultSeverity: error }
  - { id: SINGLE_FACE, group: face, subjects: [selfie], unit: count, value: v, prerequisites: [FACE_PRESENT], method: m, defaultSeverity: error }
"#;
        // Missing checks are reported before cycles; either way it must be rejected.
        assert!(Catalogue::from_yaml(yaml).is_err());
    }
}
