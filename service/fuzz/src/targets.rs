//! Fuzz targets, their seed corpora, mutators and result invariants.

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use face_core::{Catalogue, Profile};
use face_imaging::codec::{self, DecodeLimits, Decoded, Format};

use crate::mutate::{self, Rng};

#[derive(Clone, Copy, Debug)]
pub enum Target {
    /// `face_imaging::codec::decode` with the production profile limits and with tight limits.
    Decode,
    /// The JPEG (and WebP) quality estimate, reached through `decode` with header-focused mutation.
    Quality,
    /// `face_core::Profile::from_yaml` against the embedded check catalogue.
    Profile,
}

#[derive(Debug)]
pub enum Outcome {
    Ok(String),
    Rejected(String),
    Panic(String),
    Violation(String),
}

impl Outcome {
    pub fn class(&self) -> &'static str {
        match self {
            Outcome::Ok(_) => "ok",
            Outcome::Rejected(_) => "rejected",
            Outcome::Panic(_) => "panic",
            Outcome::Violation(_) => "violation",
        }
    }

    /// Coarse identity of a result, used to keep inputs that reach something new.
    pub fn signature(&self) -> String {
        let text = match self {
            Outcome::Ok(s) | Outcome::Rejected(s) | Outcome::Panic(s) | Outcome::Violation(s) => s,
        };
        let stripped: String = text
            .chars()
            .filter(|c| !c.is_ascii_digit())
            .take(160)
            .collect();
        format!("{}:{stripped}", self.class())
    }
}

/// The limits of the shipped `standard` profile (10 MiB, 40 MP), with the default allocation cap.
fn production_limits() -> DecodeLimits {
    DecodeLimits::new(10 * 1024 * 1024, 40_000_000, &Format::ALL)
}

/// Tight limits that the 48x40 seeds pass and slightly larger declared sizes do not.
fn tight_limits() -> DecodeLimits {
    let mut l = DecodeLimits::new(64 * 1024, 2_000, &Format::ALL);
    l.max_alloc = 1 << 20;
    l
}

impl Target {
    pub fn from_name(name: &str) -> Option<Target> {
        match name {
            "decode" => Some(Target::Decode),
            "quality" => Some(Target::Quality),
            "profile" => Some(Target::Profile),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Target::Decode => "decode",
            Target::Quality => "quality",
            Target::Profile => "profile",
        }
    }

    pub fn seeds(self, repo: &Path) -> Vec<Vec<u8>> {
        let fuzz = repo.join("service").join("fuzz").join("seeds");
        match self {
            Target::Decode => {
                let mut seeds = read_dir(&fuzz.join("image"), &["jpg", "png", "webp"]);
                let mut photos = read_dir(&repo.join("testdata").join("images"), &["jpg"]);
                photos.extend(read_dir(&repo.join("testdata").join("documents"), &["jpg"]));
                // In-memory PNG re-encodings of small crops of the public-domain fixtures.
                for photo in photos.iter().take(3) {
                    if let Some(crop) = small_crop(photo, 96)
                        && let Ok(png) = codec::encode_png(&crop)
                    {
                        seeds.push(png);
                    }
                }
                seeds.extend(photos);
                seeds
            }
            Target::Quality => {
                let mut seeds = read_dir(&fuzz.join("image"), &["jpg", "webp"]);
                let photos = read_dir(&repo.join("testdata").join("images"), &["jpg"]);
                if let Some(crop) = photos.first().and_then(|p| small_crop(p, 64)) {
                    for q in [1u8, 10, 25, 50, 75, 90, 100] {
                        if let Ok(jpeg) = codec::encode_jpeg(&crop, q) {
                            seeds.push(jpeg);
                        }
                    }
                }
                seeds
            }
            Target::Profile => {
                let mut seeds = read_dir(&fuzz.join("profile"), &["yaml"]);
                seeds.extend(read_dir(&repo.join("profiles"), &["yaml"]));
                seeds
            }
        }
    }

    pub fn mutate(self, rng: &mut Rng, mut data: Vec<u8>, corpus: &[Vec<u8>]) -> Vec<u8> {
        match self {
            Target::Decode | Target::Quality => {
                let format = codec::sniff(&data);
                let structured = rng.chance(if matches!(self, Target::Quality) {
                    0.7
                } else {
                    0.4
                });
                match format {
                    Some(Format::Jpeg) if structured => mutate::jpeg(rng, &mut data),
                    Some(Format::Png) if structured => mutate::png(rng, &mut data),
                    Some(Format::Webp) if structured => mutate::webp(rng, &mut data),
                    _ => {
                        let focus = if matches!(self, Target::Quality) {
                            1024
                        } else {
                            2048
                        };
                        mutate::havoc(rng, &mut data, corpus, focus, 12 << 20);
                    }
                }
                // Repair container checksums/sizes often enough to reach the inner parsers.
                match codec::sniff(&data) {
                    Some(Format::Png) if rng.chance(0.8) => mutate::png_fix_crc(&mut data),
                    Some(Format::Webp) if rng.chance(0.5) => mutate::webp_fix_riff(&mut data),
                    _ => {}
                }
                data
            }
            Target::Profile => {
                let mut text = String::from_utf8_lossy(&data).into_owned();
                mutate::yaml(rng, &mut text, corpus);
                text.into_bytes()
            }
        }
    }

    pub fn exec(self, data: &[u8]) -> Outcome {
        match self {
            Target::Decode => {
                let first = decode_checked(data, &production_limits());
                if matches!(first, Outcome::Violation(_)) {
                    return first;
                }
                let second = decode_checked(data, &tight_limits());
                match (&first, &second) {
                    (_, Outcome::Violation(_)) => second,
                    // Tight limits must never accept what production limits reject as corrupt
                    // or unsupported (they only differ in size limits).
                    (Outcome::Rejected(a), Outcome::Ok(_)) if !a.starts_with("too-") => {
                        Outcome::Violation(format!(
                            "tight limits accepted an input production limits rejected: {a}"
                        ))
                    }
                    _ => first,
                }
            }
            Target::Quality => decode_checked(data, &production_limits()),
            Target::Profile => match std::str::from_utf8(data) {
                Err(_) => Outcome::Rejected("not utf-8".into()),
                Ok(text) => match Profile::from_yaml(text, Catalogue::embedded()) {
                    Err(e) => Outcome::Rejected(first_line(&e.to_string())),
                    Ok(p) => profile_invariants(&p),
                },
            },
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines().take(2).collect::<Vec<_>>().join(" ")
}

fn decode_checked(data: &[u8], limits: &DecodeLimits) -> Outcome {
    match codec::decode(data, limits) {
        Err(e) => {
            let kind = match &e {
                codec::DecodeError::TooManyBytes { .. } => "too-many-bytes",
                codec::DecodeError::TooManyPixels { .. } => "too-many-pixels",
                codec::DecodeError::UnsupportedFormat { .. } => "unsupported",
                codec::DecodeError::Corrupt { .. } => "corrupt",
            };
            Outcome::Rejected(format!("{kind}: {e}"))
        }
        Ok(d) => decoded_invariants(data, limits, &d),
    }
}

fn decoded_invariants(data: &[u8], limits: &DecodeLimits, d: &Decoded) -> Outcome {
    let (w, h) = (d.image.width(), d.image.height());
    let mut problems = Vec::new();
    if w == 0 || h == 0 {
        problems.push(format!("decoded an empty {w}x{h} image"));
    }
    if u64::from(w) * u64::from(h) > limits.max_pixels {
        problems.push(format!("{w}x{h} exceeds max_pixels {}", limits.max_pixels));
    }
    if data.len() > limits.max_bytes {
        problems.push("accepted more than max_bytes".into());
    }
    if d.image.as_raw().len() != w as usize * h as usize * 3 {
        problems.push("pixel buffer size does not match dimensions".into());
    }
    if codec::sniff(data) != Some(d.format) || !limits.formats.contains(&d.format) {
        problems.push(format!(
            "format {:?} does not match the sniffed format",
            d.format
        ));
    }
    match d.jpeg_quality {
        Some(q) if !(q.is_finite() && (0.0..=100.0).contains(&q)) => {
            problems.push(format!("quality estimate {q} outside [0, 100]"));
        }
        Some(q) if d.format == Format::Png && q != 100.0 => {
            problems.push(format!("PNG quality {q} != 100"))
        }
        None if d.format == Format::Png => problems.push("PNG quality missing".into()),
        _ => {}
    }
    if problems.is_empty() {
        let q = d.jpeg_quality.map_or("none".to_owned(), |q| {
            format!("{}", (q / 10.0).floor() * 10.0)
        });
        Outcome::Ok(format!("{} q~{q}", d.format))
    } else {
        Outcome::Violation(problems.join("; "))
    }
}

fn profile_invariants(p: &Profile) -> Outcome {
    let mut problems = Vec::new();
    let sections = [
        ("selfie", &p.selfie.checks),
        ("document", &p.document.checks),
        ("risk", &p.verification.risk.checks),
    ];
    for (section, checks) in sections {
        for (id, s) in checks {
            if !s.enabled {
                continue;
            }
            if s.min.is_none() && s.max.is_none() {
                problems.push(format!("{section}.{id}: enabled without bounds"));
            }
            for b in [s.min, s.max].into_iter().flatten() {
                if !b.is_finite() {
                    problems.push(format!("{section}.{id}: non-finite bound"));
                }
            }
            if let (Some(a), Some(b)) = (s.min, s.max)
                && a > b
            {
                problems.push(format!("{section}.{id}: min > max"));
            }
        }
    }
    for op in &p.verification.operating_points {
        if !(-1.0..=1.0).contains(&op.threshold) || !(op.target_fmr > 0.0 && op.target_fmr <= 0.1) {
            problems.push(format!("operating point {} out of range", op.id));
        }
    }
    if p.verification.operating_point(None).is_none() {
        problems.push("default operating point missing".into());
    }
    if p.input.formats.is_empty() || p.input.max_bytes < 1024 || p.input.max_pixels < 10_000 {
        problems.push("input limits invalid".into());
    }
    if !(0.0..=0.5).contains(&p.face_counting.min_inter_eye_distance_ratio) {
        problems.push("minInterEyeDistanceRatio out of range".into());
    }
    if problems.is_empty() {
        Outcome::Ok("valid profile".into())
    } else {
        Outcome::Violation(problems.join("; "))
    }
}

fn read_dir(dir: &Path, exts: &[&str]) -> Vec<Vec<u8>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        })
        .collect();
    paths.sort();
    paths.iter().filter_map(|p| std::fs::read(p).ok()).collect()
}

fn small_crop(jpeg: &[u8], size: u32) -> Option<face_imaging::RgbImage> {
    let d = codec::decode(jpeg, &production_limits()).ok()?;
    let (w, h) = (d.image.width(), d.image.height());
    let s = size.min(w).min(h);
    face_imaging::transform::crop_pixels(&d.image, (w - s) / 2, (h - s) / 3, s, s)
}

// ------------------------------------------------------------------------------- selftest

/// Hand-written edge cases, run once each with timing and allocation figures.
pub fn selftest(panic_msg: &Arc<Mutex<Option<String>>>) -> i32 {
    let mut cases: Vec<(String, Target, Vec<u8>)> = Vec::new();

    // Images: header-declared sizes far beyond the data.
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(0x7FFF_FFFFu32.to_be_bytes());
    png.extend(0x7FFF_FFFFu32.to_be_bytes());
    png.extend([8, 2, 0, 0, 0, 0, 0, 0, 0]);
    mutate::png_fix_crc(&mut png);
    cases.push(("png 2^31x2^31 header only".into(), Target::Decode, png));
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(6000u32.to_be_bytes());
    png.extend(6000u32.to_be_bytes());
    png.extend([16, 6, 0, 0, 1, 0, 0, 0, 0]); // 16-bit RGBA, Adam7: 36 MP, ~288 MB decoded
    mutate::png_fix_crc(&mut png);
    cases.push((
        "png 6000x6000 rgba16 interlaced header only".into(),
        Target::Decode,
        png,
    ));
    let jpeg_sof = |w: u16, h: u16| {
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
        j.extend(h.to_be_bytes());
        j.extend(w.to_be_bytes());
        j.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1, 0xFF, 0xD9]);
        j
    };
    cases.push((
        "jpeg 65535x65535 SOF only".into(),
        Target::Decode,
        jpeg_sof(65535, 65535),
    ));
    cases.push((
        "jpeg 6000x6000 SOF only".into(),
        Target::Decode,
        jpeg_sof(6000, 6000),
    ));
    let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
    webp.extend([0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // 16M x 16M canvas
    mutate::webp_fix_riff(&mut webp);
    cases.push(("webp VP8X 2^24 canvas".into(), Target::Decode, webp));
    let mut dqt16 = vec![0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x83, 0x10];
    dqt16.extend(std::iter::repeat_n([0xFFu8, 0xFF], 64).flatten());
    dqt16.extend(jpeg_sof(8, 8)[2..].iter());
    cases.push(("jpeg 16-bit DQT all 65535".into(), Target::Quality, dqt16));

    // Profiles: alias expansion and nesting depth.
    let mut laughs = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
    for k in 1..9 {
        laughs.push_str(&format!(
            "a{k}: &a{k} [{}]\n",
            vec![format!("*a{}", k - 1); 10].join(", ")
        ));
    }
    cases.push((
        "yaml billion laughs (10^9 nodes)".into(),
        Target::Profile,
        laughs.into_bytes(),
    ));
    for depth in [1_000usize, 10_000, 100_000] {
        let nest = format!("name: {}{}\n", "[".repeat(depth), "]".repeat(depth));
        cases.push((
            format!("yaml {depth}-deep flow sequence"),
            Target::Profile,
            nest.into_bytes(),
        ));
        let nest = format!("name: {}1{}\n", "{a: ".repeat(depth), "}".repeat(depth));
        cases.push((
            format!("yaml {depth}-deep flow mapping"),
            Target::Profile,
            nest.into_bytes(),
        ));
    }
    let block: String = (0..2_000)
        .map(|k| format!("{}- \n", " ".repeat(k)))
        .collect();
    cases.push((
        "yaml 2000-deep block sequence".into(),
        Target::Profile,
        block.into_bytes(),
    ));

    let mut failures = 0;
    for (name, target, data) in cases {
        eprint!("{name:<48} ");
        // 1 MiB stack, like the Windows main thread that loads profiles at startup.
        let msg = panic_msg.clone();
        let handle = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || {
                let start = Instant::now();
                let base = crate::alloc::reset();
                let r = panic::catch_unwind(AssertUnwindSafe(|| target.exec(&data)));
                let (peak, _) = crate::alloc::window(base);
                let r = r.unwrap_or_else(|_| {
                    Outcome::Panic(msg.lock().unwrap().take().unwrap_or_default())
                });
                (r, start.elapsed(), peak)
            });
        let (r, t, peak) = handle.expect("spawn").join().expect("join");
        if matches!(r, Outcome::Panic(_) | Outcome::Violation(_)) {
            failures += 1;
        }
        let mut shown = format!("{r:?}");
        shown.truncate(140);
        eprintln!(
            "{:>8.1} ms  peak {:>7.1} MiB  {shown}",
            t.as_secs_f64() * 1e3,
            peak as f64 / 1048576.0
        );
    }
    i32::from(failures > 0)
}
