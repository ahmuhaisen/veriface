//! Pipeline-level golden tests (`testdata/golden/pipeline/*.json`, format v1): detections,
//! landmarks, landmarks5, blendshapes, pose, segmentation, with the tolerances stored in each
//! golden. Plus measurement sanity checks on the test images.
//!
//! Skips (prints `SKIPPED: ...`) when the ONNX Runtime library, the models or the goldens are
//! absent.

use face_core::CheckId;
use face_imaging::RgbImage;
use face_imaging::geom::{Point, Rect};
use face_imaging::transform::PixelRect;
use face_models::test_support::{load_mask, load_rgb, runtime, skip, testdata_dir};
use face_models::{Blendshape, Detection, FaceDetector, Point3, ids};
use face_quality::{
    Analysis, AnalysisOptions, Analyzer, AnalyzerConfig, CanonicalFace, PoseEstimator,
};
use serde_json::Value;

struct Golden {
    id: String,
    json: Value,
}

fn goldens() -> Option<Vec<Golden>> {
    let dir = testdata_dir().join("golden").join("pipeline");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        skip(&format!("no pipeline goldens in {}", dir.display()));
        return None;
    };
    let mut out: Vec<Golden> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .map(|p| {
            let json: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            let version = json["formatVersion"].as_u64();
            assert!(
                matches!(version, Some(1 | 2)),
                "{}: unsupported golden format {version:?}",
                p.display()
            );
            Golden {
                id: p.file_stem().unwrap().to_string_lossy().into_owned(),
                json,
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    if out.is_empty() {
        skip("no pipeline goldens");
        return None;
    }
    Some(out)
}

fn image_of(g: &Golden) -> RgbImage {
    load_rgb(&testdata_dir().join(g.json["image"]["file"].as_str().unwrap()))
}

fn f(v: &Value) -> f32 {
    v.as_f64().unwrap() as f32
}

fn point(v: &Value) -> Point {
    Point::new(f(&v[0]), f(&v[1]))
}

fn landmarks_of(face: &Value) -> Vec<Point3> {
    face["landmarks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| Point3::new(f(&p[0]), f(&p[1]), f(&p[2])))
        .collect()
}

fn tol(g: &Golden, path: &[&str]) -> f32 {
    let mut v = &g.json["tolerances"];
    for k in path {
        v = &v[*k];
    }
    f(v)
}

fn analyzer() -> Option<Analyzer> {
    Some(Analyzer::new(runtime()?, AnalyzerConfig::default()).unwrap())
}

/// Golden faces use the weak-perspective `pose` once the lab regenerates them (they then also carry
/// `poseMediapipeMatrix`, informational). Older goldens hold MediaPipe's perspective angles, which
/// the contract no longer uses: their `pose` is not compared.
fn has_weak_perspective_pose(face: &Value) -> bool {
    face.get("poseMediapipeMatrix").is_some()
}

#[test]
fn pose_fit_matches_the_reference_and_the_invariants() {
    let Some(goldens) = goldens() else { return };
    let canonical_path = face_models::test_support::models_dir().join("mp-canonical-face.json");
    let Ok(bytes) = std::fs::read(&canonical_path) else {
        skip("no canonical face asset");
        return;
    };
    let est = PoseEstimator::new(CanonicalFace::from_json(&bytes).unwrap());
    let pose_of = |id: &str| -> Option<face_quality::Pose> {
        let g = goldens.iter().find(|g| g.id == id)?;
        let face = g.json["faces"].as_array()?.first()?;
        Some(est.fit(&landmarks_of(face))?.pose)
    };
    // Reference angles (regenerated goldens only): same landmarks, same fit.
    let (mut compared, mut worst) = (0, 0.0f32);
    for g in &goldens {
        for face in g.json["faces"].as_array().unwrap() {
            if !has_weak_perspective_pose(face) {
                continue;
            }
            let full = est.fit(&landmarks_of(face)).expect("pose fit");
            let fit = full.pose;
            // Documents: `pose` is in the rectified document frame; the fit of the input-image
            // landmarks is `poseInputFrame`.
            let want = face
                .get("poseInputFrame")
                .filter(|v| !v.is_null())
                .unwrap_or(&face["pose"]);
            let d = (fit.yaw - f(&want["yaw"]))
                .abs()
                .max((fit.pitch - f(&want["pitch"])).abs())
                .max((fit.roll - f(&want["roll"])).abs());
            worst = worst.max(d);
            compared += 1;
            assert!(d < 0.05, "{}: pose {fit:?} vs reference {want}", g.id);
            if let Some(pf) = face
                .get("poseFit")
                .filter(|_| face["poseFrame"] != "document")
            {
                let scale = pf["scalePxPerCm"].as_f64().unwrap();
                let resid = pf["weightedRmsResidualPx"].as_f64().unwrap();
                assert!(
                    (full.scale - scale).abs() <= 1e-3 * scale + 1e-3,
                    "{}: scale {} vs {scale}",
                    g.id,
                    full.scale
                );
                assert!(
                    (full.residual_px - resid).abs() <= 1e-2,
                    "{}: residual {} vs {resid}",
                    g.id,
                    full.residual_px
                );
            }
        }
    }
    if compared == 0 {
        skip(
            "goldens predate the weak-perspective pose (no `poseMediapipeMatrix`); reference angles not compared",
        );
    } else {
        println!(
            "weak-perspective pose vs reference on {compared} golden faces: worst {worst:.4} deg"
        );
    }
    // Sign invariants of the derived fixtures, on the golden landmarks.
    let mut checked = 0;
    for g in &goldens {
        let Some(inv) = g.json.get("invariants").filter(|v| !v.is_null()) else {
            continue;
        };
        let other = inv
            .get("comparedWith")
            .or_else(|| inv.get("parent"))
            .and_then(Value::as_str)
            .unwrap();
        let (Some(child), Some(parent)) = (pose_of(&g.id), pose_of(other)) else {
            continue;
        };
        let t = f(&inv["toleranceDeg"]);
        let check = |name: &str, rule: &str, c: f32, p: f32| {
            let want = match rule {
                "negated" => -p,
                "equal" => p,
                r if r.starts_with("plus ") => p + r[5..].parse::<f32>().unwrap(),
                r => panic!("unknown invariant rule {r}"),
            };
            assert!(
                (c - want).abs() <= t,
                "{}: {name} {c:.2} vs expected {want:.2} ({rule})",
                g.id
            );
        };
        check("yaw", inv["yaw"].as_str().unwrap(), child.yaw, parent.yaw);
        check(
            "pitch",
            inv["pitch"].as_str().unwrap(),
            child.pitch,
            parent.pitch,
        );
        check(
            "roll",
            inv["roll"].as_str().unwrap(),
            child.roll,
            parent.roll,
        );
        println!(
            "{}: invariants vs {other} hold (pose {child:?}, other {parent:?})",
            g.id
        );
        checked += 1;
    }
    assert!(checked > 0, "no invariant fixtures found");
}

fn iou(a: &Rect, b: &Rect) -> f32 {
    a.iou(b)
}

fn rect_of(v: &Value) -> Rect {
    Rect::new(f(&v[0]), f(&v[1]), f(&v[2]), f(&v[3]))
}

fn check_detections(g: &Golden, which: &str, ours: &[Detection]) -> Result<(), String> {
    let want = g.json["detections"][which].as_array().unwrap();
    // Count exact, except for detections whose score is within the score tolerance of the 0.5
    // threshold: decoder and warp rounding can put those on either side of it.
    let score_tol = tol(g, &["detection", "score"]);
    let borderline = |s: f32| (s - 0.5).abs() <= score_tol;
    let ours: Vec<&Detection> = if ours.len() > want.len() {
        let mut kept: Vec<&Detection> = ours.iter().filter(|d| !borderline(d.score)).collect();
        if kept.len() < want.len() {
            kept = ours.iter().collect();
        }
        if kept.len() != ours.len() {
            println!(
                "{}/{which}: ignoring {} borderline detection(s) the golden does not have",
                g.id,
                ours.len() - kept.len()
            );
        }
        kept
    } else {
        ours.iter().collect()
    };
    let missing_borderline = want.len() > ours.len()
        && want.iter().filter(|w| borderline(f(&w["score"]))).count() >= want.len() - ours.len();
    if ours.len() != want.len() && !missing_borderline {
        return Err(format!(
            "{which}: {} detections, golden {}",
            ours.len(),
            want.len()
        ));
    }
    if ours.len() != want.len() {
        println!(
            "{}/{which}: golden has {} borderline detection(s) we do not",
            g.id,
            want.len() - ours.len()
        );
        return Ok(());
    }
    let mut ours = ours;
    ours.sort_by(|a, b| b.bbox.width.total_cmp(&a.bbox.width));
    for (d, w) in ours.iter().zip(want) {
        let gb = rect_of(&w["box"]);
        let edges = [
            (d.bbox.x - gb.x).abs(),
            (d.bbox.y - gb.y).abs(),
            (d.bbox.right() - gb.right()).abs(),
            (d.bbox.bottom() - gb.bottom()).abs(),
        ];
        let box_ok = iou(&d.bbox, &gb) >= tol(g, &["detection", "boxIoU"])
            || edges
                .iter()
                .all(|e| *e <= tol(g, &["detection", "boxEdgePx"]));
        if !box_ok {
            return Err(format!("{which}: box {:?} vs {gb:?}", d.bbox));
        }
        if (d.score - f(&w["score"])).abs() > tol(g, &["detection", "score"]) {
            return Err(format!("{which}: score {} vs {}", d.score, w["score"]));
        }
        let kps = w["keypoints"].as_array().unwrap();
        let iod = point(&kps[0]).distance(point(&kps[1]));
        for (k, kp) in kps.iter().enumerate() {
            let e = d.keypoints[k].distance(point(kp));
            if e > tol(g, &["detection", "keypointsIodFrac"]) * iod {
                return Err(format!(
                    "{which}: keypoint {k} off by {e:.2} px (IOD {iod:.1})"
                ));
            }
        }
    }
    Ok(())
}

/// Compare our primary face with the golden one. Returns a summary line and the list of
/// tolerance violations (empty = pass).
fn check_face(g: &Golden, a: &Analysis, face: &Value) -> (String, Vec<String>) {
    let mut bad = Vec::new();
    let Some(lm) = a.landmarks.as_ref() else {
        return (String::new(), vec!["no landmarks".to_owned()]);
    };
    let want = landmarks_of(face);
    let iod = f(&face["iod"]);
    let errs: Vec<f32> = lm
        .points
        .iter()
        .zip(&want)
        .map(|(p, q)| p.xy().distance(q.xy()))
        .collect();
    let mean = errs.iter().sum::<f32>() / errs.len() as f32;
    let max = errs.iter().copied().fold(0.0, f32::max);
    if mean > tol(g, &["landmarks", "meanIodFrac"]) * iod
        || max > tol(g, &["landmarks", "maxIodFrac"]) * iod
    {
        bad.push(format!(
            "landmarks mean {:.4} max {:.4} IOD",
            mean / iod,
            max / iod
        ));
    }
    let face_geom = a.face.as_ref().unwrap();
    let mut worst5 = 0.0f32;
    for (k, p) in face["landmarks5"].as_array().unwrap().iter().enumerate() {
        let e = face_geom.landmarks5[k].distance(point(p)) / iod;
        worst5 = worst5.max(e);
        if e > tol(g, &["landmarks5", "iodFrac"]) {
            bad.push(format!("landmarks5[{k}] off by {e:.4} IOD"));
        }
    }
    let blend = a.blendshapes.as_ref().unwrap();
    let (mut worst_blend, mut worst_name) = (0.0f32, "");
    for b in Blendshape::ALL.iter().skip(1) {
        let e = (blend.get(*b) - f(&face["blendshapes"][b.name()])).abs();
        if e > worst_blend {
            worst_blend = e;
            worst_name = b.name();
        }
    }
    // Golden tolerance: `abs` for whole-image faces, `absCropSourced` for faces MediaPipe found
    // through the full-range crop (recropped here from our own detection).
    let crop_sourced = face["source"] == "mediapipe-full-range-crop";
    let blend_tol = if crop_sourced {
        g.json["tolerances"]["blendshapes"]
            .get("absCropSourced")
            .map(f)
            .unwrap_or_else(|| tol(g, &["blendshapes", "abs"]))
    } else {
        tol(g, &["blendshapes", "abs"])
    };
    if worst_blend > blend_tol {
        let msg =
            format!("blendshape {worst_name} off by {worst_blend:.3} (tolerance {blend_tol})");
        println!("{}: EXCEEDS GOLDEN TOLERANCE: {msg}", g.id);
        bad.push(msg);
    }
    // Illumination reference values (OFIQ linear luma over the face-skin region).
    let mut ill_note = String::new();
    if let Some(ill) = face.get("illumination").filter(|v| !v.is_null()) {
        for (key, id) in [
            ("LUMINANCE_MEAN", CheckId::LuminanceMean),
            ("UNDER_EXPOSURE", CheckId::UnderExposure),
            ("OVER_EXPOSURE", CheckId::OverExposure),
        ] {
            let want = ill[key].as_f64().unwrap();
            let t = g.json["tolerances"]["illumination"][key].as_f64().unwrap();
            match a.measurements.value(id) {
                Some(v) => {
                    ill_note.push_str(&format!(" {key} {v:.4}/{want:.4}"));
                    if (v - want).abs() > t {
                        bad.push(format!("{key} {v:.4} vs golden {want:.4} (tolerance {t})"));
                    }
                }
                None => bad.push(format!("{key} not measured (golden {want})")),
            }
        }
    }
    let pose = a.pose.as_ref().unwrap().pose;
    let wp = &face["pose"];
    let dp = [
        (pose.yaw - f(&wp["yaw"])).abs(),
        (pose.pitch - f(&wp["pitch"])).abs(),
        (pose.roll - f(&wp["roll"])).abs(),
    ];
    // Only goldens with the weak-perspective reference pose are comparable (contract change).
    if has_weak_perspective_pose(face)
        && (dp[0] > tol(g, &["pose", "yawDeg"])
            || dp[1] > tol(g, &["pose", "pitchDeg"])
            || dp[2] > tol(g, &["pose", "rollDeg"]))
    {
        bad.push(format!("pose {pose:?} vs {wp}"));
    }
    let mut seg_note = String::new();
    if let Some(gs) = face["segmentation"].as_object() {
        let seg = a.segmentation.as_ref().unwrap();
        let r: Vec<u32> = gs["roi"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        let golden_roi = PixelRect {
            x: r[0],
            y: r[1],
            width: r[2],
            height: r[3],
        };
        // The ROI follows the iris landmarks (side = 6 IOD): allow what the landmarks5 tolerance
        // allows (each iris within 0.02 IOD -> side within 0.24 IOD), plus rounding.
        let roi_tol = (0.25 * iod).ceil() as i64 + 2;
        let ours = seg.roi;
        let diffs = [
            i64::from(ours.x) - i64::from(golden_roi.x),
            i64::from(ours.y) - i64::from(golden_roi.y),
            i64::from(ours.width) - i64::from(golden_roi.width),
            i64::from(ours.height) - i64::from(golden_roi.height),
        ];
        if diffs.iter().any(|d| d.abs() > roi_tol) {
            bad.push(format!("segmentation ROI {ours:?} vs {golden_roi:?}"));
        }
        // Mask agreement over the image pixels both ROIs cover.
        let (mw, mh, mask) = load_mask(
            &testdata_dir()
                .join("golden")
                .join(gs["maskFile"].as_str().unwrap()),
        );
        assert_eq!((mw, mh), (golden_roi.width, golden_roi.height));
        let (x0, y0) = (ours.x.max(golden_roi.x), ours.y.max(golden_roi.y));
        let x1 = (ours.x + ours.width).min(golden_roi.x + golden_roi.width);
        let y1 = (ours.y + ours.height).min(golden_roi.y + golden_roi.height);
        let (mut same, mut n) = (0usize, 0usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let a = seg.class(x - ours.x, y - ours.y) as u8;
                let b = mask[((y - golden_roi.y) * golden_roi.width + (x - golden_roi.x)) as usize];
                n += 1;
                same += usize::from(a == b);
            }
        }
        let agree = same as f32 / n.max(1) as f32;
        if agree < tol(g, &["segmentation", "maskAgreementMin"]) {
            bad.push(format!("mask agreement {agree:.4}"));
        }
        let fr = seg.fractions();
        let mut worst_fr = 0.0f32;
        for (k, name) in [
            "background",
            "hair",
            "body-skin",
            "face-skin",
            "clothes",
            "others",
        ]
        .iter()
        .enumerate()
        {
            let e = (fr[k] - f(&gs["classFractions"][*name])).abs();
            worst_fr = worst_fr.max(e);
            if e > tol(g, &["segmentation", "classFractionAbs"]) {
                bad.push(format!("class fraction {name} off by {e:.4}"));
            }
        }
        seg_note = format!(
            "mask agreement {agree:.4}{} class fractions within {worst_fr:.4}",
            if ours == golden_roi {
                " (same ROI),"
            } else {
                " (ROI differs by a few px),"
            }
        );
    }
    (
        format!(
            "landmarks mean {:.4} max {:.4} IOD; landmarks5 {:.4} IOD; pose d = ({:.2}, {:.2}, {:.2}) deg{}; blendshapes within {worst_blend:.3} ({worst_name}); {seg_note};{ill_note}",
            mean / iod,
            max / iod,
            worst5,
            dp[0],
            dp[1],
            dp[2],
            if has_weak_perspective_pose(face) {
                ""
            } else {
                " (golden pose predates the contract change: not compared)"
            }
        ),
        bad,
    )
}

#[test]
fn pipeline_matches_the_goldens() {
    let Some(goldens) = goldens() else { return };
    let Some(analyzer) = analyzer() else { return };
    let rt = runtime().unwrap();
    let short =
        FaceDetector::new(rt.session(ids::DETECTOR_SHORT).unwrap(), Default::default()).unwrap();
    let mut failures = Vec::new();
    let mut our_poses: Vec<(String, face_quality::Pose)> = Vec::new();
    for g in &goldens {
        let image = image_of(g);
        // Detections (both models, whole image).
        let full = analyzer.detect_faces(&image).unwrap();
        if let Err(e) = check_detections(g, "full", &full) {
            failures.push(format!("{}: {e}", g.id));
        }
        if let Err(e) = check_detections(g, "short", &short.detect(&image).unwrap()) {
            failures.push(format!("{}: {e}", g.id));
        }
        // Primary face through the selfie pipeline.
        let a = analyzer
            .analyze_selfie(&image, &AnalysisOptions::default())
            .unwrap();
        if let Some(p) = a.pose.as_ref() {
            our_poses.push((g.id.clone(), p.pose));
        }
        match g.json["faces"].as_array().unwrap().first() {
            None => {
                if a.landmarks.as_ref().is_some_and(|l| l.presence >= 0.5) {
                    failures.push(format!("{}: golden has no face, we found one", g.id));
                } else {
                    println!("{}: no face (as golden)", g.id);
                }
            }
            Some(face) if face["source"] == "document-upright-crop" => {
                // Produced by the rectify-then-landmark document path (face-document: orientation
                // search, rectification, then `analyze_portrait` on the upright crop). The
                // whole-image selfie path is not comparable (and cannot see upside-down portraits).
                println!(
                    "{}: document-upright-crop face, compared by the document pipeline (selfie path found a face: {})",
                    g.id,
                    a.landmarks.is_some()
                );
            }
            Some(face) => {
                let (note, bad) = check_face(g, &a, face);
                println!("{}: {:?} {note}", g.id, a.landmark_source);
                for e in bad {
                    failures.push(format!(
                        "{}: {e} (source {:?}, golden {})",
                        g.id, a.landmark_source, face["source"]
                    ));
                }
            }
        }
    }
    // Invariants through our own pipeline (canvas position invariance, flips, roll).
    for g in &goldens {
        let Some(inv) = g.json.get("invariants").filter(|v| !v.is_null()) else {
            continue;
        };
        let other = inv
            .get("comparedWith")
            .or_else(|| inv.get("parent"))
            .and_then(Value::as_str)
            .unwrap();
        let (Some(a), Some(b)) = (
            our_poses.iter().find(|(n, _)| n == &g.id),
            our_poses.iter().find(|(n, _)| n == other),
        ) else {
            continue;
        };
        let (c, p) = (a.1, b.1);
        let t = f(&inv["toleranceDeg"]);
        for (name, cv, pv) in [
            ("yaw", c.yaw, p.yaw),
            ("pitch", c.pitch, p.pitch),
            ("roll", c.roll, p.roll),
        ] {
            let rule = inv[name].as_str().unwrap();
            let want = match rule {
                "negated" => -pv,
                "equal" => pv,
                r if r.starts_with("plus ") => pv + r[5..].parse::<f32>().unwrap(),
                r => panic!("unknown invariant rule {r}"),
            };
            if (cv - want).abs() > t {
                failures.push(format!(
                    "{}: pipeline {name} {cv:.2} vs {want:.2} ({rule} of {other})",
                    g.id
                ));
            }
        }
        println!(
            "{}: pipeline invariants vs {other}: ours {c:?}, other {p:?}",
            g.id
        );
    }
    for f in &failures {
        println!("FAIL {f}");
    }
    assert!(
        failures.is_empty(),
        "{} golden mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Measurements on the test images: printed for the report, with relational checks the fixtures
/// were made for (blur, glare, low resolution, flips).
#[test]
fn measurements_behave_on_the_fixtures() {
    let Some(goldens) = goldens() else { return };
    let Some(analyzer) = analyzer() else { return };
    let checks = face_quality::subject_checks(face_core::Subject::Selfie);
    let mut table: Vec<(String, Analysis)> = Vec::new();
    for g in &goldens {
        let image = image_of(g);
        let a = analyzer
            .analyze_selfie(&image, &AnalysisOptions::default())
            .unwrap();
        // Every selfie check has a measurement (value or undetermined).
        for id in &checks {
            assert!(
                a.measurements.get(*id).is_some(),
                "{}: {id} not measured",
                g.id
            );
        }
        table.push((g.id.clone(), a));
    }
    let v = |id: &str, c: CheckId| -> Option<f64> {
        table
            .iter()
            .find(|(n, _)| n == id)
            .and_then(|(_, a)| a.measurements.value(c))
    };
    let header = [
        CheckId::InterEyeDistance,
        CheckId::HeadPoseYaw,
        CheckId::HeadPosePitch,
        CheckId::HeadPoseRoll,
        CheckId::EyesVisible,
        CheckId::EyesOpen,
        CheckId::MouthClosed,
        CheckId::ExpressionNeutral,
        CheckId::FaceOcclusion,
        CheckId::Eyeglasses,
        CheckId::LuminanceMean,
        CheckId::UnderExposure,
        CheckId::OverExposure,
        CheckId::DynamicRange,
        CheckId::IlluminationUniformity,
        CheckId::Sharpness,
        CheckId::BackgroundUniformity,
        CheckId::HeadInFrame,
    ];
    println!(
        "{:28} {}",
        "image",
        header
            .iter()
            .map(|c| format!("{:>8}", &c.as_str()[..c.as_str().len().min(8)]))
            .collect::<String>()
    );
    for (name, a) in &table {
        let row: String = header
            .iter()
            .map(|c| match a.measurements.value(*c) {
                Some(x) => format!("{x:>8.3}"),
                None => format!("{:>8}", "-"),
            })
            .collect();
        println!("{name:28} {row}");
    }
    // Relational expectations from the fixtures.
    if let (Some(blur), Some(sharp)) = (
        v("doc-menon-blur", CheckId::Sharpness),
        v("doc-menon", CheckId::Sharpness),
    ) {
        assert!(
            blur < sharp,
            "blurred document should be less sharp: {blur} vs {sharp}"
        );
    }
    // The glare fixture's spot lies on the portrait background, not on the face skin, so
    // OVER_EXPOSURE (face skin) need not rise; it must not fall.
    if let (Some(glare), Some(clean)) = (
        v("doc-watkins-glare", CheckId::OverExposure),
        v("doc-watkins", CheckId::OverExposure),
    ) {
        assert!(
            glare >= clean,
            "glare must not lower over-exposure: {glare} vs {clean}"
        );
    }
    if let (Some(a), Some(b)) = (
        v("nasa-glover", CheckId::HeadPoseYaw),
        v("nasa-glover-flip", CheckId::HeadPoseYaw),
    ) {
        assert!((a + b).abs() < 2.0, "flip should negate yaw: {a} {b}");
    }
    if let (Some(a), Some(b)) = (
        v("nasa-glover", CheckId::HeadPoseRoll),
        v("nasa-glover-rollcw15", CheckId::HeadPoseRoll),
    ) {
        assert!((b - a - 15.0).abs() < 2.0, "roll fixture: {a} -> {b}");
    }
    for (name, a) in &table {
        if name.starts_with("nasa-") && a.measurements.value(CheckId::EyesVisible).is_some() {
            let ev = a.measurements.value(CheckId::EyesVisible).unwrap();
            assert!(ev > 0.8, "{name}: eyes visible {ev}");
        }
    }
}

#[test]
fn output_crops_frame_the_face() {
    let Some(goldens) = goldens() else { return };
    let Some(analyzer) = analyzer() else { return };
    let Some(g) = goldens.iter().find(|g| g.id == "nasa-glover") else {
        return;
    };
    let image = image_of(g);
    let a = analyzer
        .analyze_selfie(&image, &AnalysisOptions::default())
        .unwrap();
    let head = a.head.unwrap();
    let rect = face_quality::output_rect(
        &head,
        face_core::OutputStyle::Portrait3x4,
        480,
        640,
        image.width(),
        image.height(),
    )
    .unwrap();
    let out = a
        .output(&image, face_core::OutputStyle::Portrait3x4, 480, 640)
        .unwrap();
    assert_eq!(out.dimensions(), (480, 640));
    // Eye midpoint at (0.50, 0.42) of the crop unless the crop had to shift.
    let ex = (head.eye_mid.x - rect.x) / rect.width;
    let ey = (head.eye_mid.y - rect.y) / rect.height;
    println!(
        "portrait-3x4 crop {rect:?}: eyes at ({ex:.3}, {ey:.3}); head {:.0} px of {:.0}",
        head.height, rect.height
    );
    assert!(
        rect.x >= 0.0
            && rect.y >= 0.0
            && rect.right() <= image.width() as f32 + 0.5
            && rect.bottom() <= image.height() as f32 + 0.5
    );
    let face_crop = a
        .output(&image, face_core::OutputStyle::FaceCrop, 360, 480)
        .unwrap();
    assert!(face_crop.width() <= 360 && face_crop.height() <= 480);
}

#[test]
fn portrait_entry_point_on_a_document() {
    let Some(goldens) = goldens() else { return };
    let Some(analyzer) = analyzer() else { return };
    let Some(g) = goldens.iter().find(|g| g.id == "doc-watkins") else {
        return;
    };
    let image = image_of(g);
    let faces = analyzer.detect_faces(&image).unwrap();
    let main = faces
        .iter()
        .max_by(|a, b| a.inter_eye_distance().total_cmp(&b.inter_eye_distance()))
        .unwrap();
    let hint = face_quality::FaceHint::from(main);
    let a = analyzer
        .analyze_portrait(&image, Some(&hint), &AnalysisOptions::default())
        .unwrap();
    let checks = face_quality::subject_checks(face_core::Subject::Portrait);
    for id in &checks {
        assert!(a.measurements.get(*id).is_some(), "{id} not measured");
    }
    assert!(a.measurements.get(CheckId::FacePresent).is_none());
    let golden_face = &g.json["faces"][0];
    let iod = f(&golden_face["iod"]);
    let e = a.face.unwrap().landmarks5[0].distance(point(&golden_face["landmarks5"][0]));
    assert!(e < 0.05 * iod, "portrait landmarks off by {e}");
    // Without a hint the largest full-range face is used.
    let b = analyzer
        .analyze_portrait(&image, None, &AnalysisOptions::default())
        .unwrap();
    assert!(b.face.is_some());
}
