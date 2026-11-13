//! Replay-risk measurements (`group: risk` in contracts/checks.yaml). These are deterministic
//! risk signals, not certified presentation-attack detection (ADR 0003).
//!
//! **Every threshold and weight in this module is PROVISIONAL.** None has been calibrated on
//! representative data; the lab replaces them through calibration on genuine pairs and on an
//! attack set (prints, screens, ID-card replays). Each check produces one value in [0, 1];
//! `face_core::evaluate` applies the profile's ranges.

mod ssim;

use face_document::FaceGeometry;
use face_document::rectify::{
    QuadCandidate, coverage, find_quads, quad_area, ramp, weighted_geometric_mean,
};
use face_imaging::RgbImage;
use face_imaging::geom::{Point, Rect};
use face_imaging::stats::{luma, magnitude_spectrum};
use face_imaging::transform::crop_pixels;

pub use ssim::ssim;

use crate::align::AlignedFace;

// ------------------------------------------------------------------ PROBE_REFERENCE_DUPLICATE

/// Similarity where the duplicate signal starts: the top of the normal genuine
/// selfie-vs-document range (PROVISIONAL; AuraFace genuine pairs of different captures rarely
/// exceed it, while a portrait against a print of itself scores 0.72-0.99 in the lab fixtures).
pub const DUPLICATE_SIMILARITY_LOW: f64 = 0.80;
/// Similarity where the duplicate signal saturates (PROVISIONAL).
pub const DUPLICATE_SIMILARITY_HIGH: f64 = 0.95;
/// SSIM of the aligned 112x112 crops where the duplicate signal starts and saturates
/// (PROVISIONAL).
pub const DUPLICATE_SSIM_LOW: f64 = 0.55;
pub const DUPLICATE_SSIM_HIGH: f64 = 0.90;
/// RMS distance (template pixels) between the two aligned five-point sets below which the
/// geometry counts as identical, and above which as different (PROVISIONAL).
pub const DUPLICATE_POINTS_IDENTICAL: f64 = 0.5;
pub const DUPLICATE_POINTS_DIFFERENT: f64 = 2.0;
/// Largest yaw/pitch difference (degrees) below which the pose counts as identical, and above
/// which as different (PROVISIONAL).
pub const DUPLICATE_POSE_IDENTICAL_DEG: f64 = 1.0;
pub const DUPLICATE_POSE_DIFFERENT_DEG: f64 = 4.0;
/// Weights of the similarity, SSIM and geometry signals (PROVISIONAL). Geometry alone (two
/// frontal captures) can never reach the default 0.5 limit.
pub const DUPLICATE_WEIGHTS: [f64; 3] = [0.5, 0.3, 0.2];

/// The signals behind `PROBE_REFERENCE_DUPLICATE`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DuplicateSignals {
    /// Cosine similarity of the embeddings.
    pub similarity: f64,
    /// SSIM of the aligned crops.
    pub ssim: f64,
    /// RMS distance of the aligned five points, in template pixels.
    pub points_rms: f64,
    /// Largest of |yaw difference| and |pitch difference|, degrees, when both poses are known.
    pub pose_delta_deg: Option<f64>,
}

impl DuplicateSignals {
    /// Gather the signals from the two aligned faces.
    pub fn measure(
        similarity: f64,
        probe: &AlignedFace,
        probe_face: &FaceGeometry,
        reference: &AlignedFace,
        reference_face: &FaceGeometry,
    ) -> Option<DuplicateSignals> {
        let ssim = ssim(&probe.image, &reference.image)?;
        let points_rms = (probe
            .points
            .iter()
            .zip(&reference.points)
            .map(|(a, b)| f64::from(a.distance(*b)).powi(2))
            .sum::<f64>()
            / 5.0)
            .sqrt();
        let pose_delta_deg = match (probe_face.pose, reference_face.pose) {
            (Some(p), Some(r)) => Some(f64::from(
                (p.yaw - r.yaw).abs().max((p.pitch - r.pitch).abs()),
            )),
            _ => None,
        };
        Some(DuplicateSignals {
            similarity,
            ssim,
            points_rms,
            pose_delta_deg,
        })
    }

    /// Likelihood (0-1) that the probe is a copy of the reference: weighted similarity, SSIM and
    /// geometry signals.
    pub fn score(&self) -> f64 {
        let similarity = ramp(
            self.similarity,
            DUPLICATE_SIMILARITY_LOW,
            DUPLICATE_SIMILARITY_HIGH,
        );
        let ssim = ramp(self.ssim, DUPLICATE_SSIM_LOW, DUPLICATE_SSIM_HIGH);
        let points = 1.0
            - ramp(
                self.points_rms,
                DUPLICATE_POINTS_IDENTICAL,
                DUPLICATE_POINTS_DIFFERENT,
            );
        let geometry = match self.pose_delta_deg {
            Some(d) => {
                let pose = 1.0
                    - ramp(
                        d,
                        DUPLICATE_POSE_IDENTICAL_DEG,
                        DUPLICATE_POSE_DIFFERENT_DEG,
                    );
                0.5 * (points + pose)
            }
            None => points,
        };
        let [ws, wi, wg] = DUPLICATE_WEIGHTS;
        round4(ws * similarity + wi * ssim + wg * geometry)
    }
}

// ------------------------------------------------------------------ PROBE_DOCUMENT_PRESENT

/// Aspect ratios (long/short) of cards, passport pages and photo prints: scored 1 inside this
/// range, falling off outside it (PROVISIONAL).
pub const PRESENCE_ASPECT_RANGE: (f64, f64) = (1.2, 1.75);
/// Face-box area over outline area that is plausible for a portrait on a card or print: full
/// score inside the inner range, zero outside the outer one (PROVISIONAL).
pub const PRESENCE_FACE_AREA_INNER: (f64, f64) = (0.012, 0.6);
pub const PRESENCE_FACE_AREA_OUTER: (f64, f64) = (0.003, 0.9);

/// Likelihood (0-1) that the selfie shows a document, card or printed photo around the face:
/// the best outline (from the document rectifier run on the selfie) that is card-shaped, backed
/// by edges, and encloses the face box at a plausible size.
pub fn document_presence(image: &RgbImage, face_box: &Rect) -> f64 {
    find_quads(image)
        .iter()
        .map(|q| presence_score(q, face_box))
        .fold(0.0, f64::max)
}

fn presence_score(q: &QuadCandidate, face: &Rect) -> f64 {
    let (lo, hi) = PRESENCE_ASPECT_RANGE;
    let off = if q.aspect < lo {
        (lo / q.aspect).ln()
    } else if q.aspect > hi {
        (q.aspect / hi).ln()
    } else {
        0.0
    };
    let shape = (-0.5 * (off / 0.1).powi(2)).exp();
    let card = weighted_geometric_mean(&[
        (shape, 0.3),
        (ramp(q.edge_support, 0.4, 0.85), 0.5),
        (ramp(q.fit, 0.8, 0.95), 0.2),
    ]);
    let enclosure = ramp(coverage(&q.quad, face), 0.85, 1.0);
    let ratio = f64::from(face.area()) / quad_area(&q.quad).max(1.0);
    let (inner, outer) = (PRESENCE_FACE_AREA_INNER, PRESENCE_FACE_AREA_OUTER);
    let size = ramp(ratio, outer.0, inner.0).min(1.0 - ramp(ratio, inner.1, outer.1));
    round4(card * enclosure * size)
}

// ------------------------------------------------------------------ PROBE_SCREEN_ARTIFACTS

/// Excess of a spectral peak over the median of its neighbourhood in the (3x3-smoothed) log
/// power spectrum, in natural-log units, where the screen-artifact signal starts and saturates
/// (PROVISIONAL, experimental check).
pub const SCREEN_PEAK_LOW: f64 = 1.5;
pub const SCREEN_PEAK_HIGH: f64 = 3.5;
/// Largest analysed patch side (native resolution, never resized).
pub const SCREEN_PATCH_MAX: usize = 256;
/// Smallest analysed patch side; smaller faces give no value.
pub const SCREEN_PATCH_MIN: usize = 32;

/// Likelihood (0-1) of screen or print recapture from isolated peaks (moire, pixel grid,
/// halftone) in the spectrum of the inner face region, analysed at native resolution. Bins on
/// the axes and on the 8-px JPEG block grid are ignored. `None` when the face is too small.
pub fn screen_artifacts(image: &RgbImage, face_box: &Rect) -> Option<f64> {
    let side = (0.7 * face_box.width.min(face_box.height)) as usize;
    let n = prev_power_of_two(side.min(SCREEN_PATCH_MAX));
    let (w, h) = (image.width() as usize, image.height() as usize);
    if n < SCREEN_PATCH_MIN || w < n || h < n {
        return None;
    }
    let c = face_box.center();
    let x0 = ((f64::from(c.x) - n as f64 / 2.0).round().max(0.0) as usize).min(w - n);
    let y0 = ((f64::from(c.y) - n as f64 / 2.0).round().max(0.0) as usize).min(h - n);
    Some(round4(ramp(
        spectral_peak(image, x0, y0, n),
        SCREEN_PEAK_LOW,
        SCREEN_PEAK_HIGH,
    )))
}

fn prev_power_of_two(v: usize) -> usize {
    if v == 0 {
        0
    } else {
        1 << (usize::BITS - 1 - v.leading_zeros())
    }
}

/// Largest excess of the smoothed log power over its local neighbourhood, among unmasked
/// mid/high frequencies of the `n` x `n` luma patch at (`x0`, `y0`).
fn spectral_peak(image: &RgbImage, x0: usize, y0: usize, n: usize) -> f64 {
    let Some(patch) = crop_pixels(image, x0 as u32, y0 as u32, n as u32, n as u32) else {
        return 0.0;
    };
    // Centred magnitude spectrum (DC at (n/2, n/2)), mean removed, Hann window.
    let Ok(spectrum) = magnitude_spectrum(&luma(&patch), true) else {
        return 0.0;
    };
    let size = n as i64;
    let half = size / 2;
    let power = |u: i64, v: i64| {
        let x = (u + half).rem_euclid(size) as u32;
        let y = (v + half).rem_euclid(size) as u32;
        f64::from(spectrum.get(x, y)).powi(2)
    };
    // Smoothed log power, indexed by frequency (u, v) in [-n/2, n/2).
    let mut log = vec![0.0; n * n];
    for (i, l) in log.iter_mut().enumerate() {
        let (u, v) = ((i % n) as i64 - half, (i / n) as i64 - half);
        let mut s = 0.0;
        for dv in -1..=1 {
            for du in -1..=1 {
                s += power(u + du, v + dv);
            }
        }
        *l = (s / 9.0 + 1e-18).ln();
    }
    let log_at = |u: i64, v: i64| {
        let x = (u + half).rem_euclid(size) as usize;
        let y = (v + half).rem_euclid(size) as usize;
        log[y * n + x]
    };

    let block = size / 8;
    let masked = |u: i64, v: i64| {
        let near_grid = |f: i64| {
            let r = f.rem_euclid(block);
            r <= 1 || r >= block - 1
        };
        u.abs() <= 1 || v.abs() <= 1 || near_grid(u) || near_grid(v)
    };
    // A peak is measured against its own neighbourhood (a 9x9 window without the central 3x3),
    // so smooth anisotropic spectra (faces, JPEG block noise) do not count, isolated peaks do.
    let (r_lo, r_hi) = ((n / 8) as f64, 0.45 * n as f64);
    let mut neighbours: Vec<f64> = Vec::with_capacity(72);
    let mut peak = 0.0f64;
    for i in 0..n * n {
        let (u, v) = ((i % n) as i64 - half, (i / n) as i64 - half);
        let r = ((u * u + v * v) as f64).sqrt();
        if r < r_lo || r > r_hi || masked(u, v) {
            continue;
        }
        neighbours.clear();
        for dv in -4i64..=4 {
            for du in -4i64..=4 {
                if du.abs().max(dv.abs()) >= 2 && !masked(u + du, v + dv) {
                    neighbours.push(log_at(u + du, v + dv));
                }
            }
        }
        if neighbours.len() < 16 {
            continue;
        }
        let mid = neighbours.len() / 2;
        let background = *neighbours.select_nth_unstable_by(mid, f64::total_cmp).1;
        let excess = log_at(u, v) - background;
        if excess.is_finite() {
            peak = peak.max(excess);
        }
    }
    peak
}

/// Round to 4 decimals (keeps reported values stable and readable).
fn round4(v: f64) -> f64 {
    (v * 1e4).round() / 1e4
}

/// The face box of a probe for the risk checks.
pub(crate) fn face_box(face: &FaceGeometry) -> Rect {
    if face.bbox.area() > 0.0 {
        return face.bbox;
    }
    // Fallback from the five points: about 2 IED wide and 2.4 IED tall around them.
    let ied = face.inter_eye_distance.max(1.0);
    let c = face.landmarks5[0].midpoint(face.landmarks5[1]);
    let centre = Point::new(c.x, c.y + 0.5 * ied);
    Rect::new(centre.x - ied, centre.y - 1.2 * ied, 2.0 * ied, 2.4 * ied)
}

#[cfg(test)]
mod tests;
