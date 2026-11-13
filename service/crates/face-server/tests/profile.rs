//! Where the time of one genuine verification goes (targeted timers). Ignored by default; run in
//! release:
//!
//! `cargo test --release -p face-server --test profile -- --ignored --nocapture`

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::time::{Duration, Instant};

use face_core::{Catalogue, CheckId, Profile};
use face_document::{
    BoxError, DetectedFace, FaceAnalysis, FaceFinder, OutputSize, PortraitAnalyzer, ReferenceKind,
};
use face_imaging::RgbImage;
use face_imaging::codec::{DecodeLimits, Format, decode};
use face_models::test_support;
use face_models::{FaceDetector, Runtime, RuntimeOptions, ids};
use face_quality::{AnalysisOptions, Analyzer, AnalyzerConfig, FaceHint};
use face_verify::{AuraFaceEmbedder, Embedder, Embedding, Subject};

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

struct TimedFinder<'a> {
    inner: &'a FaceDetector,
    calls: Cell<usize>,
    time: Cell<Duration>,
    pixels: RefCell<Vec<(u32, u32)>>,
}

impl FaceFinder for TimedFinder<'_> {
    fn find_faces(&self, image: &RgbImage) -> Result<Vec<DetectedFace>, BoxError> {
        let t = Instant::now();
        let r = self.inner.find_faces(image);
        self.time.set(self.time.get() + t.elapsed());
        self.calls.set(self.calls.get() + 1);
        self.pixels.borrow_mut().push(image.dimensions());
        r
    }
}

struct TimedAnalyzer<'a> {
    analyzer: &'a Analyzer,
    options: AnalysisOptions,
    calls: Cell<usize>,
    stages: RefCell<Vec<(&'static str, Duration)>>,
    total: Cell<Duration>,
}

impl PortraitAnalyzer for TimedAnalyzer<'_> {
    fn analyze_portrait(
        &self,
        crop: &RgbImage,
        face: &DetectedFace,
    ) -> Result<FaceAnalysis, BoxError> {
        let t = Instant::now();
        let hint = FaceHint {
            bbox: face.bbox,
            right_eye: face.eyes[0],
            left_eye: face.eyes[1],
        };
        let a = self
            .analyzer
            .analyze_portrait(crop, Some(&hint), &self.options)?;
        self.total.set(self.total.get() + t.elapsed());
        self.calls.set(self.calls.get() + 1);
        self.stages.borrow_mut().extend(a.timings.iter().copied());
        let geometry = a.face.map(|f| face_document::FaceGeometry {
            bbox: f.bbox,
            landmarks5: f.landmarks5,
            inter_eye_distance: f.inter_eye_distance,
            pose: f.pose.map(|p| face_document::Pose {
                yaw: p.yaw,
                pitch: p.pitch,
                roll: p.roll,
            }),
        });
        Ok(FaceAnalysis {
            measurements: a.measurements,
            face: geometry,
        })
    }
}

struct TimedEmbedder<'a> {
    inner: &'a dyn Embedder,
    time: Cell<Duration>,
}

impl Embedder for TimedEmbedder<'_> {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn embed_batch(&self, aligned: &[&RgbImage]) -> Result<Vec<Embedding>, BoxError> {
        let t = Instant::now();
        let r = self.inner.embed_batch(aligned);
        self.time.set(self.time.get() + t.elapsed());
        r
    }
}

fn enabled(settings: &face_core::CheckSettings) -> Vec<CheckId> {
    settings
        .iter()
        .filter(|(_, s)| s.enabled)
        .map(|(id, _)| *id)
        .collect()
}

fn relaxed() -> Profile {
    let text = std::fs::read_to_string(test_support::repo_dir().join("profiles/standard.yaml"))
        .unwrap()
        .replace("\r\n", "\n");
    let mut p = Profile::from_yaml(&text, Catalogue::embedded()).unwrap();
    p.document
        .checks
        .get_mut(&CheckId::InterEyeDistance)
        .unwrap()
        .min = Some(40.0);
    p
}

#[test]
#[ignore = "profiling; run explicitly in release"]
fn profile_one_genuine_verification() {
    let Some(lib) = test_support::ort_library() else {
        test_support::skip("no ONNX Runtime");
        return;
    };
    face_models::init_onnxruntime(&lib, None).unwrap();
    // PROFILE_OVERRIDES=1: auraface-v1 and mp-selfie-multiclass at 2 intra-op threads.
    let mut options = RuntimeOptions::default();
    if std::env::var_os("PROFILE_OVERRIDES").is_some() {
        for id in [ids::EMBEDDER, ids::SEGMENTER] {
            options.intra_op_threads_per_model.insert(id.to_owned(), 2);
        }
    }
    println!(
        "intra-op overrides: {:?}",
        options.intra_op_threads_per_model
    );
    let runtime = Arc::new(Runtime::load(&test_support::models_dir(), &options).unwrap());
    runtime.warm_up().unwrap();
    let analyzer = Analyzer::new(&runtime, AnalyzerConfig::default()).unwrap();
    let embedder = AuraFaceEmbedder::from_session(runtime.session(ids::EMBEDDER).unwrap()).unwrap();
    let profile = relaxed();
    let td = test_support::testdata_dir();
    let limits = DecodeLimits::new(64 << 20, 100_000_000, &Format::ALL);
    let probe_bytes = std::fs::read(td.join("images/nasa-watkins.jpg")).unwrap();
    let reference_bytes = std::fs::read(td.join("documents/doc-watkins.jpg")).unwrap();

    for round in 0..3 {
        println!("\n=== round {round}");
        let t = Instant::now();
        let probe = decode(&probe_bytes, &limits).unwrap();
        let reference = decode(&reference_bytes, &limits).unwrap();
        println!("decode both                 {:7.1} ms", ms(t.elapsed()));

        let t = Instant::now();
        let selfie = analyzer
            .analyze_selfie(
                &probe.image,
                &AnalysisOptions {
                    min_inter_eye_distance_ratio: 0.02,
                    compression_quality: probe.jpeg_quality.map(f64::from),
                    checks: Some(enabled(&profile.selfie.checks)),
                },
            )
            .unwrap();
        println!(
            "probe analyze_selfie        {:7.1} ms  {:?}",
            ms(t.elapsed()),
            selfie
                .timings
                .iter()
                .map(|(s, d)| format!("{s}={:.0}", ms(*d)))
                .collect::<Vec<_>>()
        );

        let t = Instant::now();
        let detection = face_document::rectify::detect_document(&reference.image);
        println!(
            "  detect_document alone     {:7.1} ms  (detected {})",
            ms(t.elapsed()),
            detection.detected()
        );

        let finder = TimedFinder {
            inner: analyzer.full_range_detector(),
            calls: Cell::new(0),
            time: Cell::new(Duration::ZERO),
            pixels: RefCell::new(Vec::new()),
        };
        let portrait_analyzer = TimedAnalyzer {
            analyzer: &analyzer,
            options: AnalysisOptions {
                min_inter_eye_distance_ratio: 0.02,
                compression_quality: None,
                checks: Some(enabled(&profile.document.checks)),
            },
            calls: Cell::new(0),
            stages: RefCell::new(Vec::new()),
            total: Cell::new(Duration::ZERO),
        };
        let t = Instant::now();
        let extracted = face_document::extract_portrait(
            &reference.image,
            ReferenceKind::Auto,
            &finder,
            &portrait_analyzer,
            None::<OutputSize>,
        )
        .unwrap();
        let total = t.elapsed();
        println!(
            "reference extract_portrait  {:7.1} ms: finder {} calls {:.1} ms on {:?}; portrait analysis {} calls {:.1} ms {:?}; rest (rectify, crops, rotations) {:.1} ms",
            ms(total),
            finder.calls.get(),
            ms(finder.time.get()),
            finder.pixels.borrow(),
            portrait_analyzer.calls.get(),
            ms(portrait_analyzer.total.get()),
            portrait_analyzer
                .stages
                .borrow()
                .iter()
                .map(|(s, d)| format!("{s}={:.0}", ms(*d)))
                .collect::<Vec<_>>(),
            ms(total - finder.time.get() - portrait_analyzer.total.get()),
        );

        let mut measurements = extracted.measurements.clone();
        measurements.set_value(
            CheckId::CompressionQuality,
            f64::from(reference.jpeg_quality.unwrap()),
        );
        let portrait = extracted.portrait.as_ref().unwrap();
        let probe_geometry = selfie.face.map(|f| face_document::FaceGeometry {
            bbox: f.bbox,
            landmarks5: f.landmarks5,
            inter_eye_distance: f.inter_eye_distance,
            pose: f.pose.map(|p| face_document::Pose {
                yaw: p.yaw,
                pitch: p.pitch,
                roll: p.roll,
            }),
        });
        let probe_subject = Subject {
            image: &probe.image,
            measurements: &selfie.measurements,
            face: probe_geometry.as_ref(),
        };
        let reference_subject = Subject::reference(portrait, &measurements);
        let timed = TimedEmbedder {
            inner: &embedder,
            time: Cell::new(Duration::ZERO),
        };
        let t = Instant::now();
        let probe_risk =
            face_verify::probe_measurements(&profile, &probe.image, probe_geometry.as_ref());
        println!("probe_measurements (risk)   {:7.1} ms", ms(t.elapsed()));
        let t = Instant::now();
        let v = face_verify::verify_with_probe_risk(
            Catalogue::embedded(),
            &profile,
            None,
            &probe_subject,
            &reference_subject,
            &timed,
            Some(&probe_risk),
        )
        .unwrap();
        let total = t.elapsed();
        println!(
            "verify_with_probe_risk()    {:7.1} ms: embed N=2 {:.1} ms, rest {:.1} ms; score {:?} outcome {:?}",
            ms(total),
            ms(timed.time.get()),
            ms(total - timed.time.get()),
            v.score,
            v.judgement.outcome
        );

        // The risk measurements one by one.
        let face_box = probe_geometry.as_ref().unwrap().bbox; // face_verify::risk::face_box (private) uses the bbox when non-empty
        let t = Instant::now();
        let dp = face_verify::risk::document_presence(&probe.image, &face_box);
        println!(
            "  risk document_presence    {:7.1} ms ({dp:.3}) on {:?}",
            ms(t.elapsed()),
            probe.image.dimensions()
        );
        let t = Instant::now();
        let sa = face_verify::risk::screen_artifacts(&probe.image, &face_box);
        println!(
            "  risk screen_artifacts     {:7.1} ms ({sa:?})",
            ms(t.elapsed())
        );
        let pa =
            face_verify::align_face(&probe.image, &probe_geometry.as_ref().unwrap().landmarks5)
                .unwrap();
        let ra = face_verify::align_face(
            &portrait.crop,
            &portrait.crop_face.as_ref().unwrap().landmarks5,
        )
        .unwrap();
        let t = Instant::now();
        let _ = face_verify::risk::DuplicateSignals::measure(
            v.score.unwrap(),
            &pa,
            probe_geometry.as_ref().unwrap(),
            &ra,
            portrait.crop_face.as_ref().unwrap(),
        );
        println!("  risk duplicate signals    {:7.1} ms", ms(t.elapsed()));

        let t = Instant::now();
        let _ = embedder.embed_batch(&[&pa.image]).unwrap();
        println!("  embed N=1                 {:7.1} ms", ms(t.elapsed()));
        let t = Instant::now();
        let _ = embedder.embed_batch(&[&pa.image, &ra.image]).unwrap();
        println!("  embed N=2                 {:7.1} ms", ms(t.elapsed()));
        let _: &Profile = &profile;
    }
}
