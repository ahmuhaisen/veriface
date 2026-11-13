//! Verification assembly (docs/architecture.md section 3.3): alignment, embeddings, score,
//! operating point, risk measurements, and the contract logic in `face_core`.

use face_core::{
    Assessment, Calibration, Catalogue, CheckId, DOCUMENT_ASSESSMENT, Measurements, OperatingPoint,
    Profile, RISK_ASSESSMENT, SELFIE_ASSESSMENT, Status, VerificationInput, VerificationJudgement,
    evaluate, judge,
};
use face_document::{BoxError, FaceGeometry, Portrait};
use face_imaging::RgbImage;

use crate::align::{AlignedFace, align_face};
use crate::embed::Embedder;
use crate::risk::{DuplicateSignals, document_presence, face_box, screen_artifacts};

/// One side of a verification: an image and its analysis. `face` must be in `image`
/// coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Subject<'a> {
    /// The image `face` refers to. Probe: the decoded selfie. Reference: the portrait crop
    /// (`face_document::Portrait::crop`); see [`Subject::reference`].
    pub image: &'a RgbImage,
    /// All measurements of the subject's assessment. Probe: selfie checks (face-quality plus
    /// the codec's input checks). Reference: `DocumentPortrait::measurements` plus
    /// `COMPRESSION_QUALITY`.
    pub measurements: &'a Measurements,
    /// Geometry of the primary face (probe) or of the selected portrait (reference).
    pub face: Option<&'a FaceGeometry>,
}

impl<'a> Subject<'a> {
    /// The verification reference for a document portrait: its crop with the crop-space
    /// geometry (`Portrait::crop_face`). The input-space `Portrait::face` is for the API only.
    pub fn reference(portrait: &'a Portrait, measurements: &'a Measurements) -> Subject<'a> {
        Subject {
            image: &portrait.crop,
            measurements,
            face: portrait.crop_face.as_ref(),
        }
    }
}

/// Everything `VerificationResult` needs apart from the request-level fields (request id,
/// profile ref, engine, timings) and the reference's `kind` / `document`, which come from
/// `face_document::DocumentPortrait`.
#[derive(Clone, Debug)]
pub struct Verification {
    /// The probe's selfie assessment (`probe`).
    pub probe: Assessment,
    /// The reference's document/portrait assessment (`reference`).
    pub reference: Assessment,
    /// The evaluated risk checks (`risk.checks`; `risk.flagged` is in `judgement`).
    pub risk: Assessment,
    /// The raw risk measurements behind `risk`.
    pub risk_measurements: Measurements,
    /// Cosine similarity of the embeddings; `None` when a required check failed (the outcome is
    /// then a retake) or when either face has no geometry.
    pub score: Option<f64>,
    /// The selected operating point (`operatingPoint`, `threshold`).
    pub operating_point: OperatingPoint,
    /// The profile's calibration (`calibration`).
    pub calibration: Calibration,
    /// Decision, outcome, risk flag and ordered reasons, from `face_core::judge`.
    pub judgement: VerificationJudgement,
}

impl Verification {
    pub fn threshold(&self) -> f64 {
        self.operating_point.threshold
    }
}

/// Why a verification could not be computed (map to `invalid-request` / `internal`).
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    /// The requested operating point is not in the profile (a client error).
    #[error("operating point `{0}` is not defined in the profile")]
    UnknownOperatingPoint(String),
    /// The embedder does not run the profile's model (a configuration error).
    #[error("profile `verification.model` is `{expected}` but the embedder runs `{actual}`")]
    ModelMismatch { expected: String, actual: String },
    #[error("face embedding failed: {0}")]
    Embedder(#[source] BoxError),
}

/// Compare a probe with a reference.
///
/// 1. Evaluate the probe (`SELFIE_ASSESSMENT`, `profile.selfie.checks`) and the reference
///    (`DOCUMENT_ASSESSMENT`, `profile.document.checks`).
/// 2. Unless a required probe or reference check failed (a retake: no score), align both faces
///    to the ArcFace template and embed them in one batch; the score is the cosine similarity.
///    The score is still computed when required checks are only undetermined (for review). No
///    score when either face has no geometry.
/// 3. Measure the risk checks enabled in `profile.verification.risk.checks` and evaluate them
///    with `RISK_ASSESSMENT`.
/// 4. `face_core::judge` with the selected operating point (`operating_point`, else the
///    profile's default) gives the decision, outcome and reasons.
pub fn verify(
    catalogue: &Catalogue,
    profile: &Profile,
    operating_point: Option<&str>,
    probe: &Subject<'_>,
    reference: &Subject<'_>,
    embedder: &dyn Embedder,
) -> Result<Verification, VerifyError> {
    verify_with_probe_risk(
        catalogue,
        profile,
        operating_point,
        probe,
        reference,
        embedder,
        None,
    )
}

/// Risk measurements that depend on the probe alone: `PROBE_DOCUMENT_PRESENT` and
/// `PROBE_SCREEN_ARTIFACTS`, for those enabled in `profile.verification.risk.checks`. `face` is
/// the probe's primary face in `image` coordinates.
///
/// The service can run this in its probe thread, in parallel with the reference analysis, and
/// pass the result to [`verify_with_probe_risk`]; the verification is then identical to
/// [`verify`].
pub fn probe_measurements(
    profile: &Profile,
    image: &RgbImage,
    face: Option<&FaceGeometry>,
) -> Measurements {
    let risk_settings = &profile.verification.risk.checks;
    let enabled = |id: CheckId| risk_settings.get(&id).is_some_and(|s| s.enabled);
    let mut risk = Measurements::new();
    if enabled(CheckId::ProbeDocumentPresent) {
        match face {
            Some(f) => risk.set_value(
                CheckId::ProbeDocumentPresent,
                document_presence(image, &face_box(f)),
            ),
            None => risk.set_undetermined(CheckId::ProbeDocumentPresent, "no probe face"),
        }
    }
    if enabled(CheckId::ProbeScreenArtifacts) {
        match face.map(|f| screen_artifacts(image, &face_box(f))) {
            Some(Some(v)) => risk.set_value(CheckId::ProbeScreenArtifacts, v),
            Some(None) => {
                risk.set_undetermined(CheckId::ProbeScreenArtifacts, "face region too small")
            }
            None => risk.set_undetermined(CheckId::ProbeScreenArtifacts, "no probe face"),
        }
    }
    risk
}

/// [`verify`] with the probe-only risk measurements already computed by
/// [`probe_measurements`] (same profile, probe image and probe face); `None` computes them here.
pub fn verify_with_probe_risk(
    catalogue: &Catalogue,
    profile: &Profile,
    operating_point: Option<&str>,
    probe: &Subject<'_>,
    reference: &Subject<'_>,
    embedder: &dyn Embedder,
    probe_risk: Option<&Measurements>,
) -> Result<Verification, VerifyError> {
    let settings = &profile.verification;
    let op = settings
        .operating_point(operating_point)
        .ok_or_else(|| {
            VerifyError::UnknownOperatingPoint(
                operating_point
                    .unwrap_or(&settings.default_operating_point)
                    .to_owned(),
            )
        })?
        .clone();
    if embedder.model_id() != settings.model {
        return Err(VerifyError::ModelMismatch {
            expected: settings.model.clone(),
            actual: embedder.model_id().to_owned(),
        });
    }

    let probe_assessment = evaluate(
        catalogue,
        SELFIE_ASSESSMENT,
        &profile.selfie.checks,
        probe.measurements,
    );
    let reference_assessment = evaluate(
        catalogue,
        DOCUMENT_ASSESSMENT,
        &profile.document.checks,
        reference.measurements,
    );

    // A failed required check makes the outcome a retake whatever the score, so the embeddings
    // are not computed (contracts/checks.yaml, verification.score).
    let retake = required_failed(&probe_assessment, &settings.required_probe_checks)
        || required_failed(&reference_assessment, &settings.required_reference_checks);
    let aligned = |s: &Subject<'_>| {
        s.face
            .filter(|_| !retake)
            .and_then(|f| align_face(s.image, &f.landmarks5))
    };
    let (probe_aligned, reference_aligned) = (aligned(probe), aligned(reference));
    let score = match (&probe_aligned, &reference_aligned) {
        (Some(p), Some(r)) => {
            let embeddings = embedder
                .embed_batch(&[&p.image, &r.image])
                .map_err(VerifyError::Embedder)?;
            match embeddings.as_slice() {
                [ep, er] => ep.cosine(er),
                other => {
                    return Err(VerifyError::Embedder(
                        format!("expected 2 embeddings, got {}", other.len()).into(),
                    ));
                }
            }
        }
        _ => None,
    };

    let risk_settings = &settings.risk.checks;
    let enabled = |id: CheckId| risk_settings.get(&id).is_some_and(|s| s.enabled);
    // Probe-only risk checks: precomputed by the caller (in its probe thread), or computed here.
    let mut risk = match probe_risk {
        Some(m) => m.clone(),
        None => probe_measurements(profile, probe.image, probe.face),
    };
    if enabled(CheckId::ProbeReferenceDuplicate) {
        if retake {
            risk.set_undetermined(
                CheckId::ProbeReferenceDuplicate,
                "no score: a required check failed",
            );
        } else {
            measure_duplicate(
                &mut risk,
                score,
                (probe_aligned.as_ref(), probe.face),
                (reference_aligned.as_ref(), reference.face),
            );
        }
    }
    let risk_assessment = evaluate(catalogue, RISK_ASSESSMENT, risk_settings, &risk);

    let judgement = judge(
        catalogue,
        &VerificationInput {
            probe: &probe_assessment,
            reference: &reference_assessment,
            risk: &risk_assessment,
            required_probe: &settings.required_probe_checks,
            required_reference: &settings.required_reference_checks,
            score,
            threshold: op.threshold,
        },
    );

    Ok(Verification {
        probe: probe_assessment,
        reference: reference_assessment,
        risk: risk_assessment,
        risk_measurements: risk,
        score,
        operating_point: op,
        calibration: settings.calibration.clone(),
        judgement,
    })
}

/// True when one of the `required` checks has status fail in `assessment`.
fn required_failed(assessment: &Assessment, required: &[CheckId]) -> bool {
    required
        .iter()
        .any(|id| assessment.status(*id) == Some(Status::Fail))
}

fn measure_duplicate(
    risk: &mut Measurements,
    score: Option<f64>,
    probe: (Option<&AlignedFace>, Option<&FaceGeometry>),
    reference: (Option<&AlignedFace>, Option<&FaceGeometry>),
) {
    let id = CheckId::ProbeReferenceDuplicate;
    let signals = match (score, probe, reference) {
        (Some(s), (Some(pa), Some(pf)), (Some(ra), Some(rf))) => {
            DuplicateSignals::measure(s, pa, pf, ra, rf)
        }
        _ => None,
    };
    match signals {
        Some(signals) => risk.set_value(id, signals.score()),
        None => risk.set_undetermined(id, "no similarity score"),
    }
}
