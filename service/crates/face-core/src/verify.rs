//! Verification decision and outcome, as specified under `verification` in
//! `contracts/checks.yaml`.

use serde::{Deserialize, Serialize};

use crate::catalogue::Catalogue;
use crate::check_id::CheckId;
use crate::evaluate::{Assessment, Severity, Status};

/// Reason code when the score is below the operating point's threshold.
pub const SCORE_BELOW_THRESHOLD: &str = "SCORE_BELOW_THRESHOLD";
/// Reason code when no similarity score could be computed.
pub const NO_SCORE: &str = "NO_SCORE";

/// Pure biometric comparison result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Match,
    NoMatch,
    Undetermined,
}

/// Recommended action for the integrating project.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Verified,
    Rejected,
    Review,
    Retake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasonSource {
    Probe,
    Reference,
    Risk,
    Decision,
}

/// One entry of `reasons` in a verification result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    pub source: ReasonSource,
    pub code: String,
}

impl Reason {
    fn check(source: ReasonSource, id: CheckId) -> Self {
        Reason {
            source,
            code: id.as_str().to_owned(),
        }
    }
}

/// Decision, outcome, risk flag and ordered reasons.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationJudgement {
    pub decision: Decision,
    pub outcome: Outcome,
    pub risk_flagged: bool,
    pub reasons: Vec<Reason>,
}

/// Inputs to [`judge`].
#[derive(Clone, Copy, Debug)]
pub struct VerificationInput<'a> {
    /// The evaluated selfie (probe) assessment.
    pub probe: &'a Assessment,
    /// The evaluated reference (document/portrait) assessment.
    pub reference: &'a Assessment,
    /// The evaluated risk checks.
    pub risk: &'a Assessment,
    /// From the profile. A required check missing from its assessment counts as undetermined.
    pub required_probe: &'a [CheckId],
    pub required_reference: &'a [CheckId],
    /// Cosine similarity, or `None` if no embedding could be computed.
    pub score: Option<f64>,
    /// Threshold of the selected operating point.
    pub threshold: f64,
}

/// Apply the verification rules.
pub fn judge(catalogue: &Catalogue, input: &VerificationInput<'_>) -> VerificationJudgement {
    let VerificationInput {
        probe,
        reference,
        risk,
        required_probe,
        required_reference,
        score,
        threshold,
    } = *input;
    let mut failed: Vec<Reason> = Vec::new();
    let mut undetermined: Vec<Reason> = Vec::new();
    for (source, assessment, required) in [
        (ReasonSource::Probe, probe, required_probe),
        (ReasonSource::Reference, reference, required_reference),
    ] {
        let mut ids: Vec<CheckId> = required.to_vec();
        ids.sort_by_key(|id| catalogue.reason_rank(*id));
        for id in ids {
            match assessment.status(id) {
                Some(Status::Pass) => {}
                Some(Status::Fail) => failed.push(Reason::check(source, id)),
                // Undetermined, skipped (should be prevented by profile validation) or missing.
                _ => undetermined.push(Reason::check(source, id)),
            }
        }
    }

    let score = score.filter(|s| s.is_finite());
    let decision = if !failed.is_empty() || !undetermined.is_empty() || score.is_none() {
        Decision::Undetermined
    } else if score.is_some_and(|s| s >= threshold) {
        Decision::Match
    } else {
        Decision::NoMatch
    };

    let mut risk_failures: Vec<CheckId> = risk
        .checks
        .iter()
        .filter(|c| c.status == Status::Fail && c.severity == Severity::Error)
        .map(|c| c.id)
        .collect();
    risk_failures.sort_by_key(|id| catalogue.reason_rank(*id));
    let risk_flagged = !risk_failures.is_empty();

    let outcome = if !failed.is_empty() {
        Outcome::Retake
    } else if decision == Decision::NoMatch {
        Outcome::Rejected
    } else if decision == Decision::Undetermined || risk_flagged {
        Outcome::Review
    } else {
        Outcome::Verified
    };

    let mut reasons = failed;
    reasons.extend(undetermined);
    // On a retake no score is expected (nothing was embedded), so NO_SCORE would be noise.
    if score.is_none() && outcome != Outcome::Retake {
        reasons.push(Reason {
            source: ReasonSource::Decision,
            code: NO_SCORE.to_owned(),
        });
    }
    if decision == Decision::NoMatch {
        reasons.push(Reason {
            source: ReasonSource::Decision,
            code: SCORE_BELOW_THRESHOLD.to_owned(),
        });
    }
    reasons.extend(
        risk_failures
            .into_iter()
            .map(|id| Reason::check(ReasonSource::Risk, id)),
    );

    VerificationJudgement {
        decision,
        outcome,
        risk_flagged,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluate::{Measurements, evaluate};
    use crate::profile::{CheckSetting, CheckSettings};
    use crate::{DOCUMENT_ASSESSMENT, RISK_ASSESSMENT, SELFIE_ASSESSMENT};

    fn on(min: Option<f64>, max: Option<f64>) -> CheckSetting {
        CheckSetting {
            enabled: true,
            severity: None,
            min,
            max,
        }
    }

    struct Fixture {
        probe: Assessment,
        reference: Assessment,
        risk: Assessment,
    }

    fn fixture(probe_ied: f64, reference_found: f64, duplicate: f64) -> Fixture {
        let cat = Catalogue::embedded();
        let mut ps = CheckSettings::new();
        ps.insert(CheckId::FacePresent, on(Some(1.0), None));
        ps.insert(CheckId::InterEyeDistance, on(Some(60.0), None));
        let mut pm = Measurements::new();
        pm.set_value(CheckId::FacePresent, 1.0);
        pm.set_value(CheckId::InterEyeDistance, probe_ied);

        let mut rs = CheckSettings::new();
        rs.insert(CheckId::PortraitFound, on(Some(1.0), None));
        let mut rm = Measurements::new();
        rm.set_value(CheckId::PortraitFound, reference_found);

        let mut ks = CheckSettings::new();
        ks.insert(CheckId::ProbeReferenceDuplicate, on(None, Some(0.5)));
        let mut km = Measurements::new();
        km.set_value(CheckId::ProbeReferenceDuplicate, duplicate);

        Fixture {
            probe: evaluate(cat, SELFIE_ASSESSMENT, &ps, &pm),
            reference: evaluate(cat, DOCUMENT_ASSESSMENT, &rs, &rm),
            risk: evaluate(cat, RISK_ASSESSMENT, &ks, &km),
        }
    }

    fn run(f: &Fixture, score: Option<f64>) -> VerificationJudgement {
        judge(
            Catalogue::embedded(),
            &VerificationInput {
                probe: &f.probe,
                reference: &f.reference,
                risk: &f.risk,
                required_probe: &[CheckId::FacePresent, CheckId::InterEyeDistance],
                required_reference: &[CheckId::PortraitFound],
                score,
                threshold: 0.38,
            },
        )
    }

    #[test]
    fn verified_when_match_and_no_risk() {
        let j = run(&fixture(120.0, 1.0, 0.1), Some(0.55));
        assert_eq!(
            (j.decision, j.outcome, j.risk_flagged),
            (Decision::Match, Outcome::Verified, false)
        );
        assert!(j.reasons.is_empty());
    }

    #[test]
    fn threshold_is_inclusive() {
        let j = run(&fixture(120.0, 1.0, 0.1), Some(0.38));
        assert_eq!(j.decision, Decision::Match);
    }

    #[test]
    fn rejected_when_no_match() {
        let j = run(&fixture(120.0, 1.0, 0.1), Some(0.1));
        assert_eq!(
            (j.decision, j.outcome),
            (Decision::NoMatch, Outcome::Rejected)
        );
        assert_eq!(
            j.reasons,
            vec![Reason {
                source: ReasonSource::Decision,
                code: SCORE_BELOW_THRESHOLD.into()
            }]
        );
    }

    #[test]
    fn review_when_match_but_risk_flagged() {
        let j = run(&fixture(120.0, 1.0, 0.9), Some(0.9));
        assert_eq!(
            (j.decision, j.outcome, j.risk_flagged),
            (Decision::Match, Outcome::Review, true)
        );
        assert_eq!(
            j.reasons,
            vec![Reason {
                source: ReasonSource::Risk,
                code: "PROBE_REFERENCE_DUPLICATE".into()
            }]
        );
    }

    #[test]
    fn retake_when_a_required_check_fails_even_if_risk_is_flagged() {
        let j = run(&fixture(30.0, 0.0, 0.9), Some(0.9));
        assert_eq!(
            (j.decision, j.outcome),
            (Decision::Undetermined, Outcome::Retake)
        );
        assert_eq!(
            j.reasons[0],
            Reason {
                source: ReasonSource::Probe,
                code: "INTER_EYE_DISTANCE".into()
            }
        );
        assert_eq!(
            j.reasons[1],
            Reason {
                source: ReasonSource::Reference,
                code: "PORTRAIT_FOUND".into()
            }
        );
    }

    #[test]
    fn review_when_no_score() {
        let j = run(&fixture(120.0, 1.0, 0.1), None);
        assert_eq!(
            (j.decision, j.outcome),
            (Decision::Undetermined, Outcome::Review)
        );
        assert_eq!(
            j.reasons,
            vec![Reason {
                source: ReasonSource::Decision,
                code: NO_SCORE.into()
            }]
        );
    }

    #[test]
    fn serialises_decision_and_outcome_like_the_contract() {
        let j = run(&fixture(120.0, 1.0, 0.1), Some(0.1));
        let json = serde_json::to_value(&j).unwrap();
        assert_eq!(json["decision"], "no_match");
        assert_eq!(json["outcome"], "rejected");
        assert_eq!(json["reasons"][0]["source"], "decision");
    }
}
