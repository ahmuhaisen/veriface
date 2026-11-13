//! Per-stage CPU timings (ignored by default). Run in release:
//! `cargo test --release -p face-quality --test timing -- --ignored --nocapture`

use std::time::{Duration, Instant};

use face_models::test_support::{load_rgb, runtime, skip, testdata_dir};
use face_models::{ImageInputSpec, TensorF32, ids};
use face_quality::{AnalysisOptions, Analyzer, AnalyzerConfig};

#[test]
#[ignore]
fn per_stage_timings() {
    let Some(rt) = runtime() else { return };
    let analyzer = Analyzer::new(rt, AnalyzerConfig::default()).unwrap();
    let names = [
        "nasa-glover",
        "nasa-almansoori",
        "nasa-cassada",
        "nasa-isaacman",
    ];
    let images: Vec<_> = names
        .iter()
        .map(|n| testdata_dir().join("images").join(format!("{n}.jpg")))
        .filter(|p| p.is_file())
        .map(|p| load_rgb(&p))
        .collect();
    if images.is_empty() {
        skip("no test images");
        return;
    }
    let opts = AnalysisOptions::default();
    for img in &images {
        analyzer.analyze_selfie(img, &opts).unwrap();
    }
    let runs = 5;
    let mut stages: Vec<(&'static str, Duration)> = Vec::new();
    let mut total = Duration::ZERO;
    for _ in 0..runs {
        for img in &images {
            let t = Instant::now();
            let a = analyzer.analyze_selfie(img, &opts).unwrap();
            total += t.elapsed();
            for (name, d) in a.timings {
                match stages.iter_mut().find(|(n, _)| *n == name) {
                    Some(e) => e.1 += d,
                    None => stages.push((name, d)),
                }
            }
        }
    }
    let n = (runs * images.len()) as u32;
    println!(
        "selfie analysis, {} images x {runs} runs, 1 intra-op thread:",
        images.len()
    );
    for (name, d) in &stages {
        println!("  {name:12} {:8.2} ms", (*d / n).as_secs_f64() * 1e3);
    }
    println!(
        "  {:12} {:8.2} ms",
        "total",
        (total / n).as_secs_f64() * 1e3
    );

    let s = rt.session(ids::EMBEDDER).unwrap();
    let spec = ImageInputSpec::from_tensor_spec(s.id(), &s.entry().io.inputs[0], None).unwrap();
    let input: TensorF32 = spec
        .to_tensor(&face_imaging::RgbImage::new(112, 112))
        .unwrap();
    s.run(std::slice::from_ref(&input)).unwrap();
    let t = Instant::now();
    for _ in 0..10 {
        s.run(std::slice::from_ref(&input)).unwrap();
    }
    println!(
        "  auraface     {:8.2} ms per face",
        t.elapsed().as_secs_f64() * 1e2
    );
}
