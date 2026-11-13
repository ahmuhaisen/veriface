//! Verification assembly tests with fake seams, and golden tests against the lab's references in
//! `testdata/golden` (skipped with a message when the files are absent).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use face_core::{
    Catalogue, CheckId, Decision, Measurement, Measurements, NO_SCORE, Outcome, Profile, Reason,
    ReasonSource, Status,
};
use face_document::testing::{
    CardPose, ColourFaceFinder, ColourPortraitAnalyzer, Rng, SyntheticFace, background, card_scene,
    selfie_measurements, synthetic_geometry,
};
use face_document::{
    BoxError, DocumentPortrait, FaceGeometry, OutputSize, Pose, ReferenceKind, extract_portrait,
};
use face_imaging::RgbImage;
use face_imaging::geom::{Affine, Point, Rect};
use image::Rgb;

use crate::risk::{DuplicateSignals, document_presence, screen_artifacts};
use crate::{
    ARCFACE_TEMPLATE, AURAFACE_V1, Embedder, Embedding, Subject, Verification, VerifyError,
    align_face, umeyama, verify,
};

const STANDARD: &str = include_str!("../../../../profiles/standard.yaml");

fn profile() -> Profile {
    Profile::from_yaml(STANDARD, Catalogue::embedded()).unwrap()
}

/// Returns scripted raw vectors, one batch per call.
struct Scripted {
    model: &'static str,
    batches: RefCell<Vec<Vec<Vec<f32>>>>,
}

impl Scripted {
    fn new(batches: Vec<Vec<Vec<f32>>>) -> Scripted {
        Scripted {
            model: AURAFACE_V1,
            batches: RefCell::new(batches),
        }
    }
}

impl Embedder for Scripted {
    fn model_id(&self) -> &str {
        self.model
    }

    fn embed_batch(&self, aligned: &[&RgbImage]) -> Result<Vec<Embedding>, BoxError> {
        let batch = self.batches.borrow_mut().remove(0);
        assert_eq!(batch.len(), aligned.len());
        assert!(aligned.iter().all(|i| i.dimensions() == (112, 112)));
        Ok(batch
            .into_iter()
            .map(|v| Embedding::from_raw(v).unwrap())
            .collect())
    }
}

/// Probe and reference vectors with the given cosine similarity.
fn pair(cosine: f32) -> Vec<Vec<f32>> {
    let mut a = vec![0.0; 512];
    a[0] = 1.0;
    let mut b = vec![0.0; 512];
    b[0] = cosine;
    b[1] = (1.0 - cosine * cosine).max(0.0).sqrt();
    vec![a, b]
}

/// A reference document (synthetic card, large enough for INTER_EYE_DISTANCE >= 60) processed
/// by face-document with the colour fakes.
fn reference() -> &'static (RgbImage, DocumentPortrait, Measurements) {
    static REFERENCE: OnceLock<(RgbImage, DocumentPortrait, Measurements)> = OnceLock::new();
    REFERENCE.get_or_init(|| {
        let pose = CardPose {
            rotation_deg: 4.0,
            tilt_x_deg: 6.0,
            tilt_y_deg: -5.0,
            width_fraction: 0.6,
            center: (0.5, 0.5),
        };
        let scene = card_scene(1800, 1400, &pose, 1.586, 70);
        let doc = extract_portrait(
            &scene.image,
            ReferenceKind::Document,
            &ColourFaceFinder,
            &ColourPortraitAnalyzer,
            Some(OutputSize {
                width: 360,
                height: 480,
            }),
        )
        .unwrap();
        let mut m = doc.measurements.clone();
        m.set_value(CheckId::CompressionQuality, 95.0);
        (scene.image, doc, m)
    })
}

/// A selfie: the synthetic face with sensor-like noise, a few degrees of pose.
fn selfie() -> (RgbImage, FaceGeometry, Measurements) {
    let mut img = background(720, 900, 71, 40.0);
    let face = SyntheticFace::new(360.0, 400.0, 110.0);
    face.draw(&mut img);
    let mut rng = Rng::new(72);
    for p in img.pixels_mut() {
        let d = rng.range(-20.0, 20.0);
        *p = Rgb(p.0.map(|v| (f64::from(v) + d).clamp(0.0, 255.0) as u8));
    }
    let mut g = synthetic_geometry(&img, face.center).unwrap();
    g.pose = Some(Pose {
        yaw: 6.0,
        pitch: -4.0,
        roll: 0.0,
    });
    let m = selfie_measurements(&img, &g);
    (img, g, m)
}

fn run(
    profile: &Profile,
    operating_point: Option<&str>,
    probe: &Subject<'_>,
    reference: &Subject<'_>,
    embedder: &dyn Embedder,
) -> Result<Verification, VerifyError> {
    verify(
        Catalogue::embedded(),
        profile,
        operating_point,
        probe,
        reference,
        embedder,
    )
}

fn reference_subject() -> Subject<'static> {
    let (_, doc, m) = reference();
    Subject::reference(doc.portrait.as_ref().expect("reference portrait"), m)
}

fn threshold(p: &Profile, operating_point: Option<&str>) -> f64 {
    p.verification
        .operating_point(operating_point)
        .unwrap()
        .threshold
}

fn reason(source: ReasonSource, code: &str) -> Reason {
    Reason {
        source,
        code: code.to_owned(),
    }
}

#[test]
fn verified_when_the_faces_match_and_no_risk_is_found() {
    let (img, g, m) = selfie();
    let probe = Subject {
        image: &img,
        measurements: &m,
        face: Some(&g),
    };
    let v = run(
        &profile(),
        None,
        &probe,
        &reference_subject(),
        &Scripted::new(vec![pair(0.62)]),
    )
    .unwrap();
    assert_eq!(v.judgement.decision, Decision::Match, "{v:#?}");
    assert_eq!(v.judgement.outcome, Outcome::Verified, "{:#?}", v.risk);
    assert!(!v.judgement.risk_flagged && v.judgement.reasons.is_empty());
    assert!((v.score.unwrap() - 0.62).abs() < 1e-6);
    assert_eq!(v.operating_point.id, "fmr-1e-4");
    assert_eq!(v.threshold(), threshold(&profile(), None));
    assert_eq!(v.calibration.id, "provisional-2026-10-01");
    for id in [
        CheckId::ProbeReferenceDuplicate,
        CheckId::ProbeDocumentPresent,
        CheckId::ProbeScreenArtifacts,
    ] {
        assert_eq!(
            v.risk.status(id),
            Some(Status::Pass),
            "{id}: {:?}",
            v.risk_measurements.get(id)
        );
    }
}

#[test]
fn the_document_portrait_submitted_as_the_selfie_is_a_duplicate() {
    let r = reference_subject();
    let face = r.face.unwrap();
    let m = selfie_measurements(r.image, face);
    let probe = Subject {
        image: r.image,
        measurements: &m,
        face: Some(face),
    };
    let v = run(
        &profile(),
        None,
        &probe,
        &r,
        &Scripted::new(vec![pair(1.0)]),
    )
    .unwrap();
    assert_eq!(
        v.risk_measurements.value(CheckId::ProbeReferenceDuplicate),
        Some(1.0)
    );
    assert_eq!(
        (v.judgement.decision, v.judgement.outcome),
        (Decision::Match, Outcome::Review)
    );
    assert!(v.judgement.risk_flagged);
    assert!(
        v.judgement
            .reasons
            .contains(&reason(ReasonSource::Risk, "PROBE_REFERENCE_DUPLICATE"))
    );
}

#[test]
fn a_photo_of_the_card_as_the_selfie_shows_a_document() {
    let (scene, doc, _) = reference();
    let portrait = doc.portrait.as_ref().unwrap();
    // The portrait's API geometry is in input (scene) coordinates.
    let face = portrait.face.as_ref().unwrap();
    let g = synthetic_geometry(scene, face.landmarks5[0].midpoint(face.landmarks5[1])).unwrap();
    let m = selfie_measurements(scene, &g);
    let probe = Subject {
        image: scene,
        measurements: &m,
        face: Some(&g),
    };
    let v = run(
        &profile(),
        None,
        &probe,
        &reference_subject(),
        &Scripted::new(vec![pair(0.9)]),
    )
    .unwrap();
    let presence = v
        .risk_measurements
        .value(CheckId::ProbeDocumentPresent)
        .unwrap();
    assert!(presence >= 0.7, "{presence}");
    assert_eq!(v.judgement.outcome, Outcome::Review);
    assert!(
        v.judgement
            .reasons
            .contains(&reason(ReasonSource::Risk, "PROBE_DOCUMENT_PRESENT"))
    );
}

#[test]
fn retake_without_a_probe_face_and_no_score() {
    let img = background(640, 480, 73, 0.0);
    let mut m = Measurements::new();
    m.set_value(CheckId::FacePresent, 0.0);
    let probe = Subject {
        image: &img,
        measurements: &m,
        face: None,
    };
    let v = run(
        &profile(),
        None,
        &probe,
        &reference_subject(),
        &Scripted::new(vec![]),
    )
    .unwrap();
    assert_eq!(v.score, None);
    assert_eq!(
        (v.judgement.decision, v.judgement.outcome),
        (Decision::Undetermined, Outcome::Retake)
    );
    assert_eq!(
        v.judgement.reasons[0],
        reason(ReasonSource::Probe, "FACE_PRESENT")
    );
    // On a retake no score is expected, so NO_SCORE is not reported.
    assert!(
        !v.judgement
            .reasons
            .contains(&reason(ReasonSource::Decision, NO_SCORE))
    );
    for id in [
        CheckId::ProbeReferenceDuplicate,
        CheckId::ProbeDocumentPresent,
    ] {
        assert!(matches!(
            v.risk_measurements.get(id),
            Some(Measurement::Undetermined(_))
        ));
        assert_eq!(v.risk.status(id), Some(Status::Undetermined));
    }
}

#[test]
fn no_score_is_computed_when_a_required_check_fails() {
    let (img, g, mut m) = selfie();
    // SHARPNESS is a required probe check (standard profile, min 0.30).
    m.set_value(CheckId::Sharpness, 0.1);
    let probe = Subject {
        image: &img,
        measurements: &m,
        face: Some(&g),
    };
    // No batches scripted: the embedder must not be called.
    let v = run(
        &profile(),
        None,
        &probe,
        &reference_subject(),
        &Scripted::new(vec![]),
    )
    .unwrap();
    assert_eq!(v.score, None);
    assert_eq!(
        (v.judgement.decision, v.judgement.outcome),
        (Decision::Undetermined, Outcome::Retake)
    );
    assert_eq!(
        v.judgement.reasons[0],
        reason(ReasonSource::Probe, "SHARPNESS")
    );
    assert!(matches!(
        v.risk_measurements.get(CheckId::ProbeReferenceDuplicate),
        Some(Measurement::Undetermined(_))
    ));
    // The other risk checks do not need embeddings and are still measured.
    assert!(
        v.risk_measurements
            .value(CheckId::ProbeDocumentPresent)
            .is_some()
    );
}

#[test]
fn the_score_is_still_computed_when_a_required_check_is_undetermined() {
    let (img, g, mut m) = selfie();
    m.set_undetermined(CheckId::Sharpness, "estimator not confident");
    let probe = Subject {
        image: &img,
        measurements: &m,
        face: Some(&g),
    };
    let v = run(
        &profile(),
        None,
        &probe,
        &reference_subject(),
        &Scripted::new(vec![pair(0.7)]),
    )
    .unwrap();
    assert!(
        (v.score.unwrap() - 0.7).abs() < 1e-6,
        "reviewers need the score"
    );
    assert_eq!(
        (v.judgement.decision, v.judgement.outcome),
        (Decision::Undetermined, Outcome::Review)
    );
}

#[test]
fn precomputed_probe_risk_gives_an_identical_verification() {
    use crate::{probe_measurements, verify_with_probe_risk};
    let (scene, doc, _) = reference();
    let face = doc.portrait.as_ref().unwrap().face.as_ref().unwrap();
    // A photo of the card as the probe, so every risk check has a non-trivial value.
    let g = synthetic_geometry(scene, face.landmarks5[0].midpoint(face.landmarks5[1])).unwrap();
    let m = selfie_measurements(scene, &g);
    let probe = Subject {
        image: scene,
        measurements: &m,
        face: Some(&g),
    };
    let p = profile();
    let direct = run(
        &p,
        None,
        &probe,
        &reference_subject(),
        &Scripted::new(vec![pair(0.9)]),
    )
    .unwrap();
    let pre = probe_measurements(&p, scene, Some(&g));
    let split = verify_with_probe_risk(
        Catalogue::embedded(),
        &p,
        None,
        &probe,
        &reference_subject(),
        &Scripted::new(vec![pair(0.9)]),
        Some(&pre),
    )
    .unwrap();
    assert_eq!(split.risk, direct.risk);
    assert_eq!(split.judgement, direct.judgement);
    assert_eq!(split.score, direct.score);
    for id in [
        CheckId::ProbeReferenceDuplicate,
        CheckId::ProbeDocumentPresent,
        CheckId::ProbeScreenArtifacts,
    ] {
        assert_eq!(
            split.risk_measurements.get(id),
            direct.risk_measurements.get(id),
            "{id}"
        );
    }
    assert!(pre.value(CheckId::ProbeDocumentPresent).unwrap() >= 0.7);
}

#[test]
fn the_operating_point_sets_the_threshold() {
    let (img, g, m) = selfie();
    let probe = Subject {
        image: &img,
        measurements: &m,
        face: Some(&g),
    };
    let r = reference_subject();
    let p = profile();
    // A score between the two operating points' thresholds.
    let (strict_t, lenient_t) = (threshold(&p, None), threshold(&p, Some("fmr-1e-3")));
    assert!(lenient_t < strict_t);
    let between = (0.5 * (strict_t + lenient_t)) as f32;
    let strict = run(&p, None, &probe, &r, &Scripted::new(vec![pair(between)])).unwrap();
    assert_eq!(
        (strict.judgement.decision, strict.judgement.outcome),
        (Decision::NoMatch, Outcome::Rejected)
    );
    assert_eq!(strict.threshold(), strict_t);
    let lenient = run(
        &p,
        Some("fmr-1e-3"),
        &probe,
        &r,
        &Scripted::new(vec![pair(between)]),
    )
    .unwrap();
    assert_eq!(lenient.judgement.decision, Decision::Match);
    assert_eq!(lenient.threshold(), lenient_t);

    let unknown = run(&p, Some("nope"), &probe, &r, &Scripted::new(vec![]));
    assert!(matches!(unknown, Err(VerifyError::UnknownOperatingPoint(id)) if id == "nope"));
    let mut other = Scripted::new(vec![]);
    other.model = "some-other-model";
    let mismatch = run(&p, None, &probe, &r, &other);
    assert!(matches!(mismatch, Err(VerifyError::ModelMismatch { .. })));
}

#[test]
fn disabled_risk_checks_are_skipped_and_not_measured() {
    let (img, g, m) = selfie();
    let probe = Subject {
        image: &img,
        measurements: &m,
        face: Some(&g),
    };
    let mut p = profile();
    p.verification
        .risk
        .checks
        .get_mut(&CheckId::ProbeScreenArtifacts)
        .unwrap()
        .enabled = false;
    let v = run(
        &p,
        None,
        &probe,
        &reference_subject(),
        &Scripted::new(vec![pair(0.6)]),
    )
    .unwrap();
    assert!(
        v.risk_measurements
            .get(CheckId::ProbeScreenArtifacts)
            .is_none()
    );
    assert_eq!(
        v.risk.status(CheckId::ProbeScreenArtifacts),
        Some(Status::Skipped)
    );
}

// ------------------------------------------------------------------------------- goldens

fn testdata() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata");
    dir.join("golden/pipeline").is_dir().then_some(dir)
}

fn load_json(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn point(v: &serde_json::Value) -> Point {
    Point::new(
        v[0].as_f64().expect("x") as f32,
        v[1].as_f64().expect("y") as f32,
    )
}

/// One face of a pipeline golden.
struct GoldenFace {
    geometry: FaceGeometry,
    matrix: Affine,
    embedding: Vec<f32>,
}

struct Golden {
    id: String,
    image_file: PathBuf,
    faces: Vec<GoldenFace>,
}

fn golden(dir: &Path, id: &str) -> Golden {
    let g = load_json(&dir.join(format!("golden/pipeline/{id}.json")));
    let faces = g["faces"]
        .as_array()
        .expect("faces")
        .iter()
        .map(|f| {
            let l5 = &f["landmarks5"];
            let landmarks5 = [0, 1, 2, 3, 4].map(|k| point(&l5[k]));
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for l in f["landmarks"].as_array().expect("landmarks") {
                let p = point(l);
                (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
            }
            let m = &f["alignment"]["matrix"];
            let at = |r: usize, c: usize| m[r][c].as_f64().expect("matrix") as f32;
            let pose = &f["pose"];
            GoldenFace {
                geometry: FaceGeometry {
                    bbox: Rect::new(x0, y0, x1 - x0, y1 - y0),
                    landmarks5,
                    inter_eye_distance: f["iod"].as_f64().expect("iod") as f32,
                    pose: Some(Pose {
                        yaw: pose["yaw"].as_f64().unwrap_or_default() as f32,
                        pitch: pose["pitch"].as_f64().unwrap_or_default() as f32,
                        roll: pose["roll"].as_f64().unwrap_or_default() as f32,
                    }),
                },
                matrix: Affine([at(0, 0), at(0, 1), at(0, 2), at(1, 0), at(1, 1), at(1, 2)]),
                embedding: f["embedding"]
                    .as_array()
                    .expect("embedding")
                    .iter()
                    .map(|v| v.as_f64().expect("embedding value") as f32)
                    .collect(),
            }
        })
        .collect();
    Golden {
        id: id.to_owned(),
        image_file: dir.join(g["image"]["file"].as_str().expect("image file")),
        faces,
    }
}

fn all_goldens(dir: &Path) -> Vec<Golden> {
    let mut ids: Vec<String> = std::fs::read_dir(dir.join("golden/pipeline"))
        .expect("golden/pipeline")
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            name.strip_suffix(".json").map(str::to_owned)
        })
        .collect();
    ids.sort();
    ids.iter().map(|id| golden(dir, id)).collect()
}

fn decode(path: &Path) -> RgbImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .to_rgb8()
}

#[test]
fn alignment_matches_the_lab_umeyama_on_every_golden_face() {
    let Some(dir) = testdata() else {
        eprintln!("SKIPPED: testdata/golden/pipeline not found (lab goldens absent)");
        return;
    };
    let mut faces = 0;
    for g in all_goldens(&dir) {
        for (i, f) in g.faces.iter().enumerate() {
            let ours = umeyama(&f.geometry.landmarks5, &ARCFACE_TEMPLATE).unwrap();
            // Compare where both transforms put the five points (template pixels).
            let worst = f
                .geometry
                .landmarks5
                .iter()
                .map(|p| ours.apply(*p).distance(f.matrix.apply(*p)))
                .fold(0.0f32, f32::max);
            assert!(worst <= 0.01, "{} face {i}: {worst} px", g.id);
            faces += 1;
        }
    }
    eprintln!("alignment parity: {faces} golden faces within 0.01 template px");
    assert!(faces > 20);
}

#[test]
fn aligned_crops_match_the_lab_crops() {
    let Some(dir) = testdata() else {
        eprintln!("SKIPPED: testdata/golden not found (lab goldens absent)");
        return;
    };
    let mut checked = 0;
    for id in [
        "doc-watkins",
        "nasa-almansoori",
        "nasa-glover",
        "nasa-hoshide",
    ] {
        let lab_crop = dir.join(format!("golden/model/{id}/auraface-v1.input.png"));
        if !lab_crop.is_file() {
            eprintln!("SKIPPED {id}: {} not found", lab_crop.display());
            continue;
        }
        let lab = decode(&lab_crop);
        let g = golden(&dir, id);
        let image = decode(&g.image_file);
        let face = &g.faces[0];
        let ours = align_face(&image, &face.geometry.landmarks5).unwrap();
        let (mut sum, mut max) = (0u64, 0u8);
        for (a, b) in ours.image.as_raw().iter().zip(lab.as_raw()) {
            let d = a.abs_diff(*b);
            sum += u64::from(d);
            max = max.max(d);
        }
        let mean = sum as f64 / lab.as_raw().len() as f64;
        eprintln!("{id}: aligned crop vs lab: mean {mean:.3}, max {max} grey levels");
        // The lab's warp tolerance (golden/README.md): mean 1, max 8 grey levels. This includes
        // JPEG decoder differences (zune-jpeg here, libjpeg in the lab).
        assert!(mean <= 1.0 && max <= 8, "{id}: mean {mean}, max {max}");
        checked += 1;
    }
    assert!(checked > 0, "no model goldens found");
}

#[test]
fn embedding_parity_with_the_lab() {
    use crate::AuraFaceEmbedder;
    use face_models::{ids, test_support};
    let Some(dir) = testdata() else {
        eprintln!("SKIPPED: testdata/golden not found (lab goldens absent)");
        return;
    };
    let Some(runtime) = test_support::runtime() else {
        return; // test_support printed the SKIPPED reason (no ONNX Runtime library or models)
    };
    let embedder = AuraFaceEmbedder::from_session(runtime.session(ids::EMBEDDER).unwrap()).unwrap();

    // Model level: the lab's own aligned crop in, the lab's embedding out.
    for id in [
        "doc-watkins",
        "nasa-almansoori",
        "nasa-glover",
        "nasa-hoshide",
    ] {
        let crop = dir.join(format!("golden/model/{id}/auraface-v1.input.png"));
        if !crop.is_file() {
            continue;
        }
        let ours = embedder.embed(&decode(&crop)).unwrap();
        let lab = Embedding::from_raw(golden(&dir, id).faces[0].embedding.clone()).unwrap();
        let cos = ours.cosine(&lab).unwrap();
        eprintln!("{id}: same aligned input, cosine {cos:.6}");
        assert!(cos >= 0.999, "{id}: cosine {cos}");
    }
    // Pipeline level: our alignment from the golden five points, then the embedding.
    let (mut faces, mut worst) = (0, f64::MAX);
    for g in all_goldens(&dir) {
        let image = decode(&g.image_file);
        for (i, f) in g.faces.iter().enumerate() {
            let aligned = align_face(&image, &f.geometry.landmarks5).unwrap();
            let ours = embedder.embed(&aligned.image).unwrap();
            let lab = Embedding::from_raw(f.embedding.clone()).unwrap();
            let cos = ours.cosine(&lab).unwrap();
            assert!(cos >= 0.995, "{} face {i}: cosine {cos}", g.id);
            (faces, worst) = (faces + 1, worst.min(cos));
        }
    }
    eprintln!("pipeline parity: {faces} golden faces, lowest cosine {worst:.6}");
}

/// An embedder that returns the lab's golden embedding for each golden face's aligned crop.
struct GoldenEmbedder(Vec<(RgbImage, Vec<f32>)>);

impl Embedder for GoldenEmbedder {
    fn model_id(&self) -> &str {
        AURAFACE_V1
    }

    fn embed_batch(&self, aligned: &[&RgbImage]) -> Result<Vec<Embedding>, BoxError> {
        aligned
            .iter()
            .map(|img| {
                let (_, v) = self
                    .0
                    .iter()
                    .find(|(crop, _)| crop == *img)
                    .ok_or("crop not among the goldens")?;
                Embedding::from_raw(v.clone()).ok_or_else(|| "zero embedding".into())
            })
            .collect()
    }
}

fn portrait_reference_measurements(face: &FaceGeometry, image: &RgbImage) -> Measurements {
    let mut m = face_document::testing::portrait_measurements(Some(face));
    m.set_value(
        CheckId::ImageResolution,
        f64::from(image.width().min(image.height())),
    );
    m.set_value(CheckId::CompressionQuality, 90.0);
    m.set_value(CheckId::DocumentDetected, 1.0);
    m.set_value(CheckId::PortraitFound, 1.0);
    m
}

#[test]
fn risk_checks_and_verification_on_the_lab_fixtures() {
    let Some(dir) = testdata() else {
        eprintln!("SKIPPED: testdata/golden/pipeline not found (lab goldens absent)");
        return;
    };
    let ids = [
        "nasa-watkins",
        "doc-watkins",
        "doc-watkins-jpeg",
        "nasa-meir",
        "nasa-moghbeli",
        "nasa-glover",
        "nasa-glover-flip",
    ];
    let fixtures: Vec<(Golden, RgbImage)> = ids
        .iter()
        .map(|id| {
            let g = golden(&dir, id);
            let image = decode(&g.image_file);
            (g, image)
        })
        .collect();
    let by_id = |id: &str| fixtures.iter().find(|(g, _)| g.id == id).expect(id);
    let embedder = GoldenEmbedder(
        fixtures
            .iter()
            .flat_map(|(g, image)| {
                g.faces.iter().map(move |f| {
                    let crop = align_face(image, &f.geometry.landmarks5).unwrap().image;
                    (crop, f.embedding.clone())
                })
            })
            .collect(),
    );

    // Single-image risk signals on real photographs.
    for (g, image) in &fixtures {
        let face = &g.faces[0].geometry;
        let presence = document_presence(image, &face.bbox);
        let screen = screen_artifacts(image, &face.bbox);
        eprintln!(
            "{:20} document present {presence:.3}  screen artifacts {screen:?}",
            g.id
        );
        if g.id.starts_with("doc-") {
            // A photographed ID card used as the selfie.
            assert!(presence >= 0.7, "{}: {presence}", g.id);
        } else {
            assert!(presence <= 0.2, "{}: {presence}", g.id);
        }
        // Real photographs, no screen in the loop (warning-level, experimental check).
        assert!(screen.is_some_and(|s| s <= 0.5), "{}: {screen:?}", g.id);
    }

    let pair = |p: &Profile, probe_id: &str, reference_id: &str| {
        let (pg, pi) = by_id(probe_id);
        let (rg, ri) = by_id(reference_id);
        let (pf, rf) = (&pg.faces[0].geometry, &rg.faces[0].geometry);
        let pm = selfie_measurements(pi, pf);
        let rm = portrait_reference_measurements(rf, ri);
        let v = verify(
            Catalogue::embedded(),
            p,
            None,
            &Subject {
                image: pi,
                measurements: &pm,
                face: Some(pf),
            },
            &Subject {
                image: ri,
                measurements: &rm,
                face: Some(rf),
            },
            &embedder,
        )
        .unwrap();
        let lab = Embedding::from_raw(pg.faces[0].embedding.clone())
            .unwrap()
            .cosine(&Embedding::from_raw(rg.faces[0].embedding.clone()).unwrap())
            .unwrap();
        if let Some(score) = v.score {
            assert!((score - lab).abs() < 1e-6);
        }
        let pa = align_face(pi, &pf.landmarks5).unwrap();
        let ra = align_face(ri, &rf.landmarks5).unwrap();
        let signals = DuplicateSignals::measure(lab, &pa, pf, &ra, rf).unwrap();
        eprintln!(
            "{probe_id} vs {reference_id}: score {:.3} -> {:?}/{:?}, duplicate {:.3} ({signals:?})",
            lab,
            v.judgement.decision,
            v.judgement.outcome,
            signals.score()
        );
        (v, signals)
    };

    // The card's portrait has an inter-eye distance of about 56 px, below the standard
    // profile's 60: a required reference check fails, so the outcome is a retake.
    let standard = profile();
    let (retake, _) = pair(&standard, "nasa-watkins", "doc-watkins");
    assert_eq!(retake.judgement.outcome, Outcome::Retake);
    assert_eq!(retake.score, None, "no score on a retake");
    assert_eq!(
        retake.judgement.reasons[0],
        reason(ReasonSource::Reference, "INTER_EYE_DISTANCE")
    );

    // With that requirement relaxed: the card was printed from this very portrait, so the probe
    // is a copy, not a new capture. It matches, and the duplicate risk sends it to review.
    let mut relaxed = profile();
    relaxed
        .document
        .checks
        .get_mut(&CheckId::InterEyeDistance)
        .unwrap()
        .min = Some(40.0);
    let (copy, _) = pair(&relaxed, "nasa-watkins", "doc-watkins");
    assert_eq!(copy.judgement.decision, Decision::Match);
    assert!(copy.judgement.risk_flagged, "{:?}", copy.risk_measurements);
    assert_eq!(copy.judgement.outcome, Outcome::Review);

    // Different people (the lab's closest impostor pair, 0.386): no duplicate signal.
    let (impostor, signals) = pair(&relaxed, "nasa-meir", "nasa-moghbeli");
    assert!(signals.score() <= 0.2, "{signals:?}");
    assert_eq!(
        impostor.risk.status(CheckId::ProbeReferenceDuplicate),
        Some(Status::Pass)
    );

    // A mirrored copy of the same photo.
    let (_, mirrored) = pair(&relaxed, "nasa-glover-flip", "nasa-glover");
    assert!(mirrored.similarity > 0.8, "{mirrored:?}");
}

/// Test-only portrait analysis on the real Face Mesh model (face-quality's analysis is not wired
/// yet): landmarks on the crop, the five points and the inter-eye distance.
struct MeshAnalyzer(face_models::FaceLandmarker);

impl MeshAnalyzer {
    fn geometry(&self, image: &RgbImage, bbox: &Rect, eyes: [Point; 2]) -> Option<FaceGeometry> {
        let roi = face_models::landmark_roi(bbox, eyes[0], eyes[1]);
        let lm = self.0.landmarks(image, &roi).ok()?;
        if lm.presence < 0.5 {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &lm.points {
            (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
        }
        Some(FaceGeometry {
            bbox: Rect::new(x0, y0, x1 - x0, y1 - y0),
            landmarks5: lm.landmarks5(),
            inter_eye_distance: lm.inter_eye_distance(),
            pose: None,
        })
    }
}

impl face_document::PortraitAnalyzer for MeshAnalyzer {
    fn analyze_portrait(
        &self,
        crop: &RgbImage,
        face: &face_document::DetectedFace,
    ) -> Result<face_document::FaceAnalysis, BoxError> {
        let geometry = self.geometry(crop, &face.bbox, face.eyes);
        Ok(face_document::FaceAnalysis {
            measurements: face_document::testing::portrait_measurements(geometry.as_ref()),
            face: geometry,
        })
    }
}

#[test]
fn end_to_end_with_the_real_models_matches_the_lab_score() {
    use crate::AuraFaceEmbedder;
    use face_models::{DetectorConfig, FaceDetector, FaceLandmarker, ids, test_support};
    let Some(dir) = testdata() else {
        eprintln!("SKIPPED: testdata/golden not found (lab goldens absent)");
        return;
    };
    let Some(runtime) = test_support::runtime() else {
        return; // test_support printed the SKIPPED reason
    };
    let session = |id: &str| runtime.session(id).unwrap();
    let full = FaceDetector::new(session(ids::DETECTOR_FULL), DetectorConfig::default()).unwrap();
    let short = FaceDetector::new(session(ids::DETECTOR_SHORT), DetectorConfig::default()).unwrap();
    let mesh = MeshAnalyzer(FaceLandmarker::new(session(ids::LANDMARKER)).unwrap());
    let embedder = AuraFaceEmbedder::from_session(session(ids::EMBEDDER)).unwrap();

    // A probe "selfie": the public-domain portrait a card was printed from.
    let probe = |id: &str| {
        let image = test_support::load_rgb(&dir.join(format!("images/{id}.jpg")));
        let detections = short.detect(&image).unwrap();
        let d = detections
            .iter()
            .max_by(|a, b| a.inter_eye_distance().total_cmp(&b.inter_eye_distance()))
            .expect("probe face");
        let face = mesh
            .geometry(&image, &d.bbox, [d.right_eye(), d.left_eye()])
            .expect("probe landmarks");
        let m = selfie_measurements(&image, &face);
        (image, face, m)
    };
    let lab_score = |a: &str, b: &str| {
        let a = Embedding::from_raw(golden(&dir, a).faces[0].embedding.clone()).unwrap();
        let b = Embedding::from_raw(golden(&dir, b).faces[0].embedding.clone()).unwrap();
        a.cosine(&b).unwrap()
    };
    // The card portraits' inter-eye distance (about 56 px) is below the standard 60.
    let mut p = profile();
    p.document
        .checks
        .get_mut(&CheckId::InterEyeDistance)
        .unwrap()
        .min = Some(40.0);
    // (probe, document, lab golden with a face for the same card, genuine). The lab finds no face
    // on the upside-down card (no detector does without the orientation search), so it is
    // compared with the upright card's score.
    let cases = [
        ("nasa-watkins", "doc-watkins", "doc-watkins", true),
        ("nasa-watkins", "doc-watkins-rot180", "doc-watkins", true),
        (
            "nasa-almansoori",
            "doc-almansoori-perspective",
            "doc-almansoori-perspective",
            true,
        ),
        (
            "nasa-watkins",
            "doc-almansoori-perspective",
            "doc-almansoori-perspective",
            false,
        ),
    ];
    for (probe_id, doc, lab_doc, genuine) in cases {
        let (probe_image, probe_face, probe_m) = probe(probe_id);
        let image = test_support::load_rgb(&dir.join(format!("documents/{doc}.jpg")));
        let r = extract_portrait(&image, ReferenceKind::Document, &full, &mesh, None).unwrap();
        let portrait = r.portrait.as_ref().expect("reference portrait");
        let mut rm = r.measurements.clone();
        rm.set_value(CheckId::CompressionQuality, 90.0);
        let v = verify(
            Catalogue::embedded(),
            &p,
            None,
            &Subject {
                image: &probe_image,
                measurements: &probe_m,
                face: Some(&probe_face),
            },
            &Subject::reference(portrait, &rm),
            &embedder,
        )
        .unwrap();
        let score = v.score.expect("score");
        let lab = lab_score(probe_id, lab_doc);
        eprintln!(
            "{probe_id} vs {doc}: score {score:.4} (lab, whole-image pipeline: {lab:.4}) -> {:?}/{:?}, duplicate {:?}",
            v.judgement.decision,
            v.judgement.outcome,
            v.risk_measurements.value(CheckId::ProbeReferenceDuplicate)
        );
        // The service path (rectify, rotate, crop, landmarks on the crop) agrees with the lab's
        // whole-image path.
        assert!(
            (score - lab).abs() <= 0.03,
            "{probe_id} vs {doc}: {score} vs {lab}"
        );
        if genuine {
            // The card is a print of the probe photo: a match, sent to review as a duplicate.
            assert_eq!(v.judgement.decision, Decision::Match);
            assert_eq!(v.judgement.outcome, Outcome::Review);
        } else {
            assert_eq!(v.judgement.decision, Decision::NoMatch);
            assert_eq!(v.judgement.outcome, Outcome::Rejected);
        }
    }
}

/// Cost of the probe-only risk checks on real photographs. Run in release:
/// `cargo test --release -p face-verify probe_risk_timing -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark; run in release with --ignored"]
fn probe_risk_timing() {
    use crate::probe_measurements;
    let Some(dir) = testdata() else {
        eprintln!("SKIPPED: testdata/golden not found (lab goldens absent)");
        return;
    };
    let p = profile();
    for id in ["nasa-watkins", "nasa-glover", "doc-watkins"] {
        let g = golden(&dir, id);
        let image = decode(&g.image_file);
        let face = &g.faces[0].geometry;
        let best = (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                std::hint::black_box(probe_measurements(&p, &image, Some(face)));
                t.elapsed().as_secs_f64() * 1e3
            })
            .fold(f64::MAX, f64::min);
        eprintln!(
            "{id} ({}x{}): probe_measurements {best:.1} ms",
            image.width(),
            image.height()
        );
    }
}
