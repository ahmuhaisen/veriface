//! Rectification on the lab's synthetic specimen-card photographs (`testdata/documents`), against
//! the ground truth in `testdata/provenance.json`. Skips with a message when the files are absent.

use std::path::{Path, PathBuf};

use face_imaging::geom::{Point, Quad};

use crate::rectify::{DETECTION_THRESHOLD, detect_document, rectified_frame};

fn testdata() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata");
    dir.join("provenance.json").is_file().then_some(dir)
}

struct Doc {
    id: String,
    file: PathBuf,
    variant: String,
    corners: [Point; 4],
}

fn documents(dir: &Path) -> Vec<Doc> {
    let text = std::fs::read_to_string(dir.join("provenance.json")).expect("provenance.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("provenance.json is JSON");
    let docs = json["documents"].as_array().expect("provenance documents");
    docs.iter()
        .map(|d| {
            let c = &d["groundTruth"]["cardCorners"];
            let corner = |k: usize| {
                Point::new(
                    c[k][0].as_f64().expect("corner x") as f32,
                    c[k][1].as_f64().expect("corner y") as f32,
                )
            };
            Doc {
                id: d["id"].as_str().expect("id").to_owned(),
                file: dir.join(d["file"].as_str().expect("file")),
                variant: d["variant"].as_str().unwrap_or_default().to_owned(),
                corners: [corner(0), corner(1), corner(2), corner(3)],
            }
        })
        .collect()
}

fn corner_error(found: &Quad, truth: &[Point; 4]) -> f32 {
    (0..4)
        .map(|shift| {
            (0..4)
                .map(|k| found.0[k].distance(truth[(k + shift) % 4]))
                .fold(0.0f32, f32::max)
        })
        .fold(f32::MAX, f32::min)
}

#[test]
fn rectifies_the_lab_specimen_cards() {
    let Some(dir) = testdata() else {
        eprintln!("SKIPPED: testdata/provenance.json not found (lab fixtures absent)");
        return;
    };
    let docs = documents(&dir);
    assert!(!docs.is_empty());
    let results: Vec<(String, String, f64, Option<f32>, f32)> = std::thread::scope(|s| {
        let handles: Vec<_> = docs
            .iter()
            .map(|doc| {
                s.spawn(move || {
                    let image = image::open(&doc.file)
                        .unwrap_or_else(|e| panic!("{}: {e}", doc.file.display()))
                        .to_rgb8();
                    let d = detect_document(&image);
                    let truth = doc.corners;
                    let diagonal = truth[0].distance(truth[2]).max(truth[1].distance(truth[3]));
                    let rel = d.detected().then(|| {
                        corner_error(&d.candidate.as_ref().unwrap().quad, &truth) / diagonal
                    });
                    if let Some(c) = d.candidate.as_ref().filter(|_| d.detected()) {
                        assert!(rectified_frame(&c.quad).is_some(), "{}", doc.id);
                    }
                    (
                        doc.id.clone(),
                        doc.variant.clone(),
                        d.confidence,
                        rel,
                        diagonal,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("worker"))
            .collect()
    });

    for (id, variant, confidence, rel, diagonal) in &results {
        eprintln!(
            "{id:28} {variant:36} confidence {confidence:.3}  corner error {}",
            rel.map_or("-".to_owned(), |r| format!(
                "{:.2}% of {diagonal:.0} px",
                100.0 * r
            ))
        );
    }
    for (id, variant, confidence, rel, _) in &results {
        if variant.starts_with("flat scan") {
            // The card fills the frame, so its own outline cannot be found; the best outline is
            // the portrait photo's frame. The rectifier cannot tell the two apart; the pipeline
            // rejects outlines whose face is too large for a document (MAX_DOCUMENT_IED_RATIO,
            // tests::flat_scan_*), then reports DOCUMENT_DETECTED low and uses the image as is.
            continue;
        }
        assert!(
            *confidence >= DETECTION_THRESHOLD,
            "{id} ({variant}): confidence {confidence}"
        );
        // Measured: at most 0.16% of the diagonal on these fixtures.
        let tolerance = if variant.contains("low resolution") || variant.contains("blur") {
            0.008
        } else {
            0.004
        };
        let rel = rel.expect("detected");
        assert!(rel <= tolerance, "{id} ({variant}): corner error {rel}");
    }
}

/// The detection as the portrait geometry (no landmarks): enough to test orientation search and
/// portrait selection with the real detector while face-quality's analysis is not wired.
struct DetectionOnly;

impl crate::PortraitAnalyzer for DetectionOnly {
    fn analyze_portrait(
        &self,
        _crop: &face_imaging::RgbImage,
        _face: &crate::DetectedFace,
    ) -> Result<crate::FaceAnalysis, crate::BoxError> {
        Ok(crate::FaceAnalysis::default())
    }
}

#[test]
fn finds_upright_portraits_on_the_lab_cards_with_the_real_detector() {
    use crate::{ProcessedAs, ReferenceKind, Rotation, extract_portrait, rectify::coverage};
    use face_core::CheckId;
    use face_models::{DetectorConfig, FaceDetector, ids, test_support};

    let Some(dir) = testdata() else {
        eprintln!("SKIPPED: testdata/provenance.json not found (lab fixtures absent)");
        return;
    };
    let Some(runtime) = test_support::runtime() else {
        return; // test_support printed the SKIPPED reason
    };
    let detector = FaceDetector::new(
        runtime
            .session(ids::DETECTOR_FULL)
            .expect("full-range detector"),
        DetectorConfig::default(),
    )
    .expect("detector");
    let text = std::fs::read_to_string(dir.join("provenance.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    for d in json["documents"].as_array().unwrap() {
        let id = d["id"].as_str().unwrap();
        let truth = &d["groundTruth"];
        let image = test_support::load_rgb(&dir.join(d["file"].as_str().unwrap()));
        let r = extract_portrait(
            &image,
            ReferenceKind::Document,
            &detector,
            &DetectionOnly,
            None,
        )
        .unwrap();
        let info = r.document.as_ref().unwrap();
        let found = r.measurements.value(CheckId::PortraitFound).unwrap();
        eprintln!(
            "{id:28} detected {} conf {:.3} orientation {:3} portraits {found}",
            info.detected,
            info.confidence,
            info.orientation_degrees.degrees()
        );
        assert_eq!(r.kind, ProcessedAs::Document);
        let mains = truth["mainPortraits"].as_u64().unwrap() as f64;
        assert_eq!(found, mains, "{id}: the ghost portrait is not a candidate");
        if mains == 0.0 {
            assert!(r.portrait.is_none(), "{id}");
            continue;
        }
        let rotation = match truth["rotationCw"].as_u64().unwrap() {
            0 => Rotation::R0,
            90 => Rotation::R270,
            180 => Rotation::R180,
            _ => Rotation::R90,
        };
        assert_eq!(info.orientation_degrees, rotation, "{id}");
        assert_eq!(
            info.detected,
            !d["variant"].as_str().unwrap().starts_with("flat scan"),
            "{id}"
        );
        // Corners in DOCUMENT order, compared with the ground truth in its own order (the
        // card's top-left, top-right, bottom-right, bottom-left), rotated cards included.
        if let Some(corners) = info.corners {
            let c = &truth["cardCorners"];
            let t = |k: usize| {
                Point::new(
                    c[k][0].as_f64().unwrap() as f32,
                    c[k][1].as_f64().unwrap() as f32,
                )
            };
            let diagonal = t(0).distance(t(2)).max(t(1).distance(t(3)));
            for (k, p) in corners.0.iter().enumerate() {
                let rel = p.distance(t(k)) / diagonal;
                assert!(rel <= 0.004, "{id}: corner {k} {p:?} vs {:?}", t(k));
            }
        }
        // The selected face lies inside one of the main portrait frames.
        let p = r.portrait.as_ref().unwrap();
        let inside = truth["mainPortraitQuads"]
            .as_array()
            .unwrap()
            .iter()
            .any(|q| {
                let pt = |k: usize| {
                    Point::new(
                        q[k][0].as_f64().unwrap() as f32,
                        q[k][1].as_f64().unwrap() as f32,
                    )
                };
                coverage(&Quad([pt(0), pt(1), pt(2), pt(3)]), &p.source_box) >= 0.8
            });
        assert!(inside, "{id}: {:?}", p.source_box);
    }
}

/// Parity and timing of the document-path outline search (`find_document_quads`, coarse to fine)
/// against the full-resolution search (`find_quads_in`) on the lab cards. Run in release:
/// `cargo test --release -p face-document document_search_parity -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark; run in release with --ignored"]
fn document_search_parity() {
    use crate::rectify::{Working, find_document_quads, find_quads_in};
    use std::time::Instant;
    let Some(dir) = testdata() else {
        eprintln!("SKIPPED: testdata/provenance.json not found (lab fixtures absent)");
        return;
    };
    let reps = 5;
    let (mut total_old, mut total_new) = (0.0, 0.0);
    let (mut worst_old, mut worst_new, mut worst_conf) = (0.0f32, 0.0f32, 0.0f64);
    eprintln!(
        "{:28} {:>8} {:>8} {:>9} {:>9} {:>7} {:>7}",
        "document", "old ms", "new ms", "old err%", "new err%", "old conf", "new conf"
    );
    for doc in documents(&dir) {
        let image = image::open(&doc.file).unwrap().to_rgb8();
        let working = Working::new(&image);
        let time = |f: &dyn Fn() -> Vec<crate::rectify::QuadCandidate>| {
            let mut best = f64::MAX;
            let mut out = Vec::new();
            for _ in 0..reps {
                let t = Instant::now();
                out = f();
                best = best.min(t.elapsed().as_secs_f64() * 1e3);
            }
            (best, out)
        };
        let (t_old, old) = time(&|| find_quads_in(&working));
        let (t_new, new) = time(&|| find_document_quads(&working));
        let truth = doc.corners;
        let diagonal = truth[0].distance(truth[2]).max(truth[1].distance(truth[3]));
        let err = |c: &[crate::rectify::QuadCandidate]| {
            c.first()
                .filter(|c| c.confidence >= DETECTION_THRESHOLD)
                .map(|c| 100.0 * corner_error(&c.quad, &truth) / diagonal)
        };
        let conf = |c: &[crate::rectify::QuadCandidate]| c.first().map_or(0.0, |c| c.confidence);
        let (eo, en) = (err(&old), err(&new));
        if let (Some(o), Some(n)) = (old.first(), new.first()) {
            let delta = (0..4)
                .map(|k| o.quad.0[k].distance(n.quad.0[k]))
                .fold(0.0f32, f32::max);
            eprintln!(
                "  {}: old-vs-new corner delta {delta:.3} px, confidence delta {:.5}",
                doc.id,
                (o.confidence - n.confidence).abs()
            );
        }
        eprintln!(
            "{:28} {t_old:8.1} {t_new:8.1} {:>9} {:>9} {:7.4} {:7.4}",
            doc.id,
            eo.map_or("-".into(), |e| format!("{e:.3}")),
            en.map_or("-".into(), |e| format!("{e:.3}")),
            conf(&old),
            conf(&new)
        );
        total_old += t_old;
        total_new += t_new;
        assert_eq!(eo.is_some(), en.is_some(), "{}: detection changed", doc.id);
        if !doc.variant.starts_with("flat scan") {
            worst_old = worst_old.max(eo.unwrap_or(0.0));
            worst_new = worst_new.max(en.unwrap_or(0.0));
            worst_conf = worst_conf.max((conf(&old) - conf(&new)).abs());
        }
    }
    eprintln!(
        "total {total_old:.1} ms -> {total_new:.1} ms; worst corner error {worst_old:.3}% -> {worst_new:.3}%; worst confidence change {worst_conf:.4}"
    );
}
