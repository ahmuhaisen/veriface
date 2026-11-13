//! Integration tests on the real models: manifest verification, session I/O, determinism, and the
//! model-level goldens in `testdata/golden/model/` (warp, raw outputs, detection decoding).
//!
//! Every test skips (prints `SKIPPED: ...` and returns) when the ONNX Runtime library, the models
//! or the goldens are absent.

use std::path::{Path, PathBuf};

use face_imaging::RgbImage;
use face_imaging::geom::{Affine, Point};
use face_imaging::transform::{PixelRect, RotatedRect};
use face_models::test_support::{
    allclose_excess, load_rgb, models_dir, read_npy, runtime, skip, testdata_dir,
};
use face_models::{
    Blendshapes, DetectorConfig, FaceDetector, FaceLandmarker, ImageInputSpec, Manifest, ModelRole,
    ReviewStatus, Runtime, Segmenter, TensorF32, ids,
};
use serde_json::Value;

fn golden_model_dirs() -> Option<Vec<PathBuf>> {
    let root = testdata_dir().join("golden").join("model");
    let Ok(entries) = std::fs::read_dir(&root) else {
        skip(&format!("no model-level goldens in {}", root.display()));
        return None;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("index.json").is_file())
        .collect();
    dirs.sort();
    if dirs.is_empty() {
        skip("no model-level goldens");
        return None;
    }
    Some(dirs)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The fixture's source image (`testdata/images/<id>.jpg` or `testdata/documents/<id>.jpg`).
fn source_image(id: &str) -> RgbImage {
    let images = testdata_dir().join("images").join(format!("{id}.jpg"));
    let docs = testdata_dir().join("documents").join(format!("{id}.jpg"));
    load_rgb(if images.is_file() { &images } else { &docs })
}

fn roi_of(v: &Value) -> RotatedRect {
    RotatedRect {
        center: Point::new(
            v["cx"].as_f64().unwrap() as f32,
            v["cy"].as_f64().unwrap() as f32,
        ),
        width: v["w"].as_f64().unwrap() as f32,
        height: v["h"].as_f64().unwrap() as f32,
        rotation: v["rotation"].as_f64().unwrap() as f32,
    }
}

/// Compare two tensor images: (max, mean) absolute grey-level difference.
fn image_diff(a: &RgbImage, b: &RgbImage) -> (u8, f64) {
    assert_eq!(a.dimensions(), b.dimensions());
    let mut max = 0u8;
    let mut sum = 0u64;
    for (x, y) in a.as_raw().iter().zip(b.as_raw()) {
        let d = x.abs_diff(*y);
        max = max.max(d);
        sum += u64::from(d);
    }
    (max, sum as f64 / a.as_raw().len() as f64)
}

fn assert_warp(name: &str, ours: &RgbImage, golden: &RgbImage) {
    let (max, mean) = image_diff(ours, golden);
    println!("{name}: warp max {max} mean {mean:.4} grey levels");
    assert!(
        max <= 8 && mean <= 1.0,
        "{name}: warp differs: max {max}, mean {mean}"
    );
}

fn assert_close(
    name: &str,
    actual: &TensorF32,
    expected: &(Vec<usize>, Vec<f32>),
    atol: f32,
    rtol: f32,
) {
    let n_actual: usize = actual.shape().iter().product();
    assert_eq!(
        n_actual,
        expected.1.len(),
        "{name}: size {:?} vs {:?}",
        actual.shape(),
        expected.0
    );
    let excess = allclose_excess(actual.data(), &expected.1, atol, rtol);
    let max_abs = actual
        .data()
        .iter()
        .zip(&expected.1)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    println!("{name}: max |diff| {max_abs:.3e} (atol {atol:e}, rtol {rtol:e})");
    assert!(
        excess <= 0.0,
        "{name}: not allclose; max |diff| {max_abs:e}"
    );
}

fn run_png(runtime: &Runtime, model: &str, png: &Path) -> Vec<TensorF32> {
    let session = runtime.session(model).unwrap();
    let spec = ImageInputSpec::from_tensor_spec(session.id(), &session.entry().io.inputs[0], None)
        .unwrap();
    let img = load_rgb(png);
    session.run(&[spec.to_tensor(&img).unwrap()]).unwrap()
}

#[test]
fn manifest_verifies_and_every_session_matches_its_io_spec() {
    let Some(rt) = runtime() else { return };
    let manifest = Manifest::load(&models_dir()).unwrap();
    assert_eq!(rt.manifest().models.len(), manifest.models.len());
    for id in [
        ids::DETECTOR_SHORT,
        ids::DETECTOR_FULL,
        ids::LANDMARKER,
        ids::BLENDSHAPES,
        ids::SEGMENTER,
        ids::EMBEDDER,
    ] {
        let s = rt.session(id).unwrap();
        assert_ne!(s.entry().review.status, ReviewStatus::Rejected);
        assert_eq!(s.inputs().len(), s.entry().io.inputs.len());
    }
    assert_eq!(
        rt.sessions_with_role(ModelRole::FaceDetector, None).len(),
        2
    );
    assert_eq!(
        rt.sessions_with_role(ModelRole::FaceDetector, Some("full-range"))[0].id(),
        ids::DETECTOR_FULL
    );
    let asset = rt.asset(ids::CANONICAL_FACE).unwrap();
    assert!(asset.starts_with(b"{"));
    for (id, t) in rt.warm_up().unwrap() {
        println!("warm-up {id}: {t:?}");
    }
}

#[test]
fn models_are_deterministic_and_shapes_match_the_manifest() {
    let Some(rt) = runtime() else { return };
    for session in rt.sessions() {
        let inputs: Vec<TensorF32> = session
            .inputs()
            .iter()
            .map(|i| {
                let shape: Vec<usize> = i.shape.iter().map(|d| (*d).max(1) as usize).collect();
                let n: usize = shape.iter().product();
                // Deterministic pseudo-random values in [-1, 1].
                let data = (0..n)
                    .map(|k| ((k as f32 * 0.618_034).fract() * 2.0) - 1.0)
                    .collect();
                TensorF32::new(shape, data).unwrap()
            })
            .collect();
        let a = session.run(&inputs).unwrap();
        let b = session.run(&inputs).unwrap();
        assert_eq!(a, b, "{} is not deterministic", session.id());
        for (out, spec) in a.iter().zip(&session.entry().io.outputs) {
            let fixed: Vec<Option<i64>> = spec.shape.iter().map(|d| d.fixed()).collect();
            assert_eq!(
                out.shape().len(),
                fixed.len(),
                "{} {}",
                session.id(),
                spec.name
            );
            for (d, f) in out.shape().iter().zip(&fixed) {
                if let Some(f) = f {
                    assert_eq!(*d as i64, *f, "{} {}", session.id(), spec.name);
                }
            }
            assert!(
                out.data().iter().all(|v| v.is_finite()),
                "{} {}",
                session.id(),
                spec.name
            );
        }
        // A wrong input shape is rejected before ONNX Runtime sees it.
        assert!(session.run(&[TensorF32::zeros(vec![1, 2, 3])]).is_err());
    }
}

#[test]
fn detectors_find_nothing_in_a_flat_image() {
    let Some(rt) = runtime() else { return };
    for id in [ids::DETECTOR_SHORT, ids::DETECTOR_FULL] {
        let det = FaceDetector::new(rt.session(id).unwrap(), DetectorConfig::default()).unwrap();
        let flat = RgbImage::from_pixel(640, 480, image::Rgb([128, 128, 128]));
        assert!(det.detect(&flat).unwrap().is_empty(), "{id}");
    }
}

#[test]
fn golden_detectors_warp_outputs_and_decoding() {
    let Some(rt) = runtime() else { return };
    let Some(dirs) = golden_model_dirs() else {
        return;
    };
    for dir in dirs {
        let index = read_json(&dir.join("index.json"));
        let id = index["image"].as_str().unwrap();
        let image = source_image(id);
        for model in [ids::DETECTOR_SHORT, ids::DETECTOR_FULL] {
            let g = &index["models"][model];
            if g.is_null() {
                continue;
            }
            let det =
                FaceDetector::new(rt.session(model).unwrap(), DetectorConfig::default()).unwrap();
            let roi = roi_of(&g["roi"]);
            // Warp.
            let (ours, to_image) = det.preprocess(&image, &roi);
            let golden_input = load_rgb(&dir.join(g["input"].as_str().unwrap()));
            assert_warp(&format!("{id}/{model}"), &ours, &golden_input);
            // Our tensor->image map is the inverse of the golden source->tensor matrix.
            let m = &g["warpMatrix"];
            let src_to_tensor = Affine([
                m[0][0].as_f64().unwrap() as f32,
                m[0][1].as_f64().unwrap() as f32,
                m[0][2].as_f64().unwrap() as f32,
                m[1][0].as_f64().unwrap() as f32,
                m[1][1].as_f64().unwrap() as f32,
                m[1][2].as_f64().unwrap() as f32,
            ]);
            let p = Point::new(37.0, 91.0);
            assert!(
                (src_to_tensor.apply(to_image.apply(p)) - p).norm() < 1e-2,
                "{id}/{model} matrix"
            );
            // Raw outputs on the golden input. Anchors whose logit is below -10 in both runs
            // (score < 5e-5) can never become a detection; in the letterbox padding their
            // activations reach thousands, and ONNX Runtime 1.28 (service) and 1.30 (lab) differ
            // there by up to ~1e-4 of the anchor's magnitude. Those anchors are compared relative to
            // their own magnitude; every live anchor uses the golden tolerance.
            let outputs = run_png(rt, model, &dir.join(g["input"].as_str().unwrap()));
            let session = rt.session(model).unwrap();
            let reg = read_npy(&dir.join(g["outputs"]["regressors"].as_str().unwrap())).unwrap();
            let cls =
                read_npy(&dir.join(g["outputs"]["classificators"].as_str().unwrap())).unwrap();
            let ours_reg = outputs[session.output_index("regressors").unwrap()].data();
            let ours_cls = outputs[session.output_index("classificators").unwrap()].data();
            let (mut live_max, mut dead_max) = (0.0f32, 0.0f32);
            for (a, (logit, ours_logit)) in cls.1.iter().zip(ours_cls).enumerate() {
                let dead = *logit < -10.0 && *ours_logit < -10.0;
                let scale = reg.1[a * 16..a * 16 + 16]
                    .iter()
                    .fold(logit.abs(), |m, v| m.max(v.abs()));
                let (atol, rtol) = if dead {
                    (1e-4 + 2e-4 * scale, 0.0)
                } else {
                    (1e-4, 1e-5)
                };
                for k in 0..16 {
                    let (o, e) = (ours_reg[a * 16 + k], reg.1[a * 16 + k]);
                    let d = (o - e).abs();
                    assert!(
                        d <= atol + rtol * e.abs(),
                        "{id}/{model}: anchor {a} coord {k}: {o} vs {e} (logit {logit})"
                    );
                    if dead {
                        dead_max = dead_max.max(d)
                    } else {
                        live_max = live_max.max(d)
                    }
                }
                let d = (ours_logit - logit).abs();
                assert!(
                    d <= atol + rtol * logit.abs(),
                    "{id}/{model}: anchor {a} logit {ours_logit} vs {logit}"
                );
            }
            println!(
                "{id}/{model}: max |diff| {live_max:.3e} on live anchors, {dead_max:.3e} on logit < -10 anchors"
            );
            // Decoding of the golden raw outputs.
            let dets = det.postprocess(&reg.1, &cls.1, &to_image);
            let want = g["decoded"].as_array().unwrap();
            assert_eq!(dets.len(), want.len(), "{id}/{model}: detection count");
            for (d, w) in dets.iter().zip(want) {
                let b: Vec<f32> = w["box"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap() as f32)
                    .collect();
                for (got, exp) in [d.bbox.x, d.bbox.y, d.bbox.width, d.bbox.height]
                    .iter()
                    .zip(&b)
                {
                    assert!(
                        (got - exp).abs() < 0.05,
                        "{id}/{model}: box {:?} vs {b:?}",
                        d.bbox
                    );
                }
                assert!((d.score - w["score"].as_f64().unwrap() as f32).abs() < 1e-4);
                for (k, kp) in w["keypoints"].as_array().unwrap().iter().enumerate() {
                    let e = Point::new(
                        kp[0].as_f64().unwrap() as f32,
                        kp[1].as_f64().unwrap() as f32,
                    );
                    assert!(
                        (d.keypoints[k] - e).norm() < 0.05,
                        "{id}/{model}: keypoint {k}"
                    );
                }
            }
            // End to end from the JPEG through our decoder, warp and the model. The reference used
            // libjpeg-turbo and OpenCV's fixed-point warp, so inputs differ by a few grey levels;
            // compare with the pipeline detection tolerance (README): score within 0.03, keypoints
            // within 0.05 of the eye distance.
            let ours = det.detect(&image).unwrap();
            assert_eq!(
                ours.len(),
                want.len(),
                "{id}/{model}: end-to-end detection count"
            );
            for (d, w) in ours.iter().zip(want) {
                let iod = d.inter_eye_distance();
                let mut worst = 0.0f32;
                for (k, kp) in w["keypoints"].as_array().unwrap().iter().enumerate() {
                    let e = Point::new(
                        kp[0].as_f64().unwrap() as f32,
                        kp[1].as_f64().unwrap() as f32,
                    );
                    worst = worst.max((d.keypoints[k] - e).norm());
                }
                let score_diff = (d.score - w["score"].as_f64().unwrap() as f32).abs();
                println!(
                    "{id}/{model}: end-to-end keypoints within {worst:.3} px (IOD {iod:.1}), score within {score_diff:.4}"
                );
                assert!(
                    worst <= 0.05 * iod,
                    "{id}/{model}: end-to-end keypoints off by {worst} px"
                );
                assert!(
                    score_diff <= 0.03,
                    "{id}/{model}: end-to-end score off by {score_diff}"
                );
            }
        }
    }
}

#[test]
fn golden_landmarker_blendshapes_segmenter_and_embedder_outputs() {
    let Some(rt) = runtime() else { return };
    let Some(dirs) = golden_model_dirs() else {
        return;
    };
    for dir in dirs {
        let index = read_json(&dir.join("index.json"));
        let id = index["image"].as_str().unwrap();
        let image = source_image(id);
        let models = &index["models"];

        if let Some(g) = models.get(ids::LANDMARKER).filter(|g| !g.is_null()) {
            let lm = FaceLandmarker::new(rt.session(ids::LANDMARKER).unwrap()).unwrap();
            let roi = roi_of(&g["roi"]);
            let png = dir.join(g["input"].as_str().unwrap());
            assert_warp(
                &format!("{id}/landmarker"),
                &lm.preprocess(&image, &roi),
                &load_rgb(&png),
            );
            let outputs = run_png(rt, ids::LANDMARKER, &png);
            let session = rt.session(ids::LANDMARKER).unwrap();
            for (name, file) in g["outputs"].as_object().unwrap() {
                let expected = read_npy(&dir.join(file.as_str().unwrap())).unwrap();
                let k = session.output_index(name).unwrap();
                let atol = if name == "landmarks" { 1e-3 } else { 1e-4 };
                assert_close(
                    &format!("{id}/landmarker/{name}"),
                    &outputs[k],
                    &expected,
                    atol,
                    1e-5,
                );
            }
            let presence = 1.0
                / (1.0
                    + (-outputs[session.output_index("presence_logit").unwrap()].data()[0]).exp());
            assert!((presence - g["presence"].as_f64().unwrap() as f32).abs() < 1e-3);
        }

        if let Some(g) = models.get(ids::BLENDSHAPES).filter(|g| !g.is_null()) {
            let bs = Blendshapes::new(rt.session(ids::BLENDSHAPES).unwrap()).unwrap();
            let (shape, points) = read_npy(&dir.join(g["input"].as_str().unwrap())).unwrap();
            let scores = bs
                .predict_points(&TensorF32::new(shape, points).unwrap())
                .unwrap();
            let expected =
                read_npy(&dir.join(g["outputs"]["blendshapes"].as_str().unwrap())).unwrap();
            let actual = TensorF32::new(vec![52], scores.0.to_vec()).unwrap();
            assert_close(&format!("{id}/blendshapes"), &actual, &expected, 1e-4, 1e-5);
        }

        if let Some(g) = models.get(ids::SEGMENTER).filter(|g| !g.is_null()) {
            let seg = Segmenter::new(rt.session(ids::SEGMENTER).unwrap()).unwrap();
            let roi = roi_of(&g["roi"]);
            let px = PixelRect {
                x: (roi.center.x - roi.width / 2.0).round() as u32,
                y: (roi.center.y - roi.height / 2.0).round() as u32,
                width: roi.width.round() as u32,
                height: roi.height.round() as u32,
            };
            let png = dir.join(g["input"].as_str().unwrap());
            assert_warp(
                &format!("{id}/segmenter"),
                &seg.preprocess(&image, &px),
                &load_rgb(&png),
            );
            let outputs = run_png(rt, ids::SEGMENTER, &png);
            let expected = read_npy(&dir.join(g["outputs"]["logits"].as_str().unwrap())).unwrap();
            assert_close(
                &format!("{id}/segmenter/logits"),
                &outputs[0],
                &expected,
                2e-2,
                1e-3,
            );
        }

        if let Some(g) = models.get(ids::EMBEDDER).filter(|g| !g.is_null()) {
            let png = dir.join(g["input"].as_str().unwrap());
            let outputs = run_png(rt, ids::EMBEDDER, &png);
            let expected =
                read_npy(&dir.join(g["outputs"]["embedding"].as_str().unwrap())).unwrap();
            assert_close(
                &format!("{id}/auraface"),
                &outputs[0],
                &expected,
                1e-4,
                1e-5,
            );
        }
    }
}

/// Per-call model timings, 1 thread vs a 2-thread per-model override (ignored; run in release:
/// `cargo test --release -p face-models --test models per_model_timings -- --ignored --nocapture`).
/// Configurations are interleaved over several rounds and the fastest round is reported, so
/// background load affects both alike.
#[test]
#[ignore]
fn per_model_timings() {
    use face_models::RuntimeOptions;
    use std::time::Instant;
    let Some(_) = runtime() else { return };
    let ids_to_time = [
        ids::SEGMENTER,
        ids::EMBEDDER,
        ids::LANDMARKER,
        ids::DETECTOR_FULL,
    ];
    let runtimes: Vec<(usize, Runtime)> = [1usize, 2]
        .into_iter()
        .map(|threads| {
            let mut opts = RuntimeOptions {
                models: Some(ids_to_time.iter().map(|s| s.to_string()).collect()),
                ..Default::default()
            };
            for id in ids_to_time {
                opts.intra_op_threads_per_model
                    .insert(id.to_owned(), threads);
            }
            (threads, Runtime::load(&models_dir(), &opts).unwrap())
        })
        .collect();
    let mut best = std::collections::BTreeMap::new();
    for _round in 0..6 {
        for (threads, rt) in &runtimes {
            for id in ids_to_time {
                let s = rt.session(id).unwrap();
                let inputs: Vec<TensorF32> = s
                    .inputs()
                    .iter()
                    .map(|i| {
                        TensorF32::zeros(i.shape.iter().map(|d| (*d).max(1) as usize).collect())
                    })
                    .collect();
                s.run(&inputs).unwrap();
                let n = 5;
                let t = Instant::now();
                for _ in 0..n {
                    s.run(&inputs).unwrap();
                }
                let ms = t.elapsed().as_secs_f64() * 1e3 / n as f64;
                let e = best.entry((id, *threads)).or_insert(f64::MAX);
                *e = e.min(ms);
            }
        }
    }
    for ((id, threads), ms) in &best {
        println!("{id:24} threads {threads}: {ms:7.1} ms/call (best of 6 rounds)");
    }
}
