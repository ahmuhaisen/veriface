//! Document-outline candidates: edges -> connected components -> convex hulls -> quadrilaterals,
//! refined by line fits on the gradient, then scored.

use face_imaging::RgbImage;
use face_imaging::geom::Quad;

use super::edges::{Gradients, dilate, hysteresis, percentile};
use super::raster::Gray;
use super::shapes::{
    Line, V2, convex_hull, is_convex, label_components, perimeter, row_extremes, signed_area,
    simplify_closed,
};
use super::warp::{downscale_area, fit_long_side};

/// Long side of the working copy used for edge analysis.
pub const WORKING_LONG_SIDE: u32 = 1024;
/// Smallest quadrilateral considered, as a fraction of the image area.
pub const MIN_AREA_FRACTION: f64 = 0.02;
/// ID-1 card aspect ratio (85.60 mm x 53.98 mm; ISO/IEC 7810).
pub const ID1_ASPECT: f64 = 85.60 / 53.98;
/// TD3 passport data page aspect ratio (125 mm x 88 mm; ICAO 9303).
pub const TD3_ASPECT: f64 = 125.0 / 88.0;

/// Gaussian blur before edge detection, in working-image pixels.
const BLUR_SIGMA: f32 = 1.5;
/// Gradient floor (grey levels per pixel) below which nothing counts as an edge.
const MIN_EDGE_STRENGTH: f32 = 4.0;
/// Quantiles of the edge-strength distribution used as Canny high thresholds; one pass each.
/// The low ones keep weak document borders (a light card on a light table) whose strength is
/// far below the printing on the card; the high ones isolate strong borders from texture.
const LEVELS: [f32; 4] = [0.15, 0.4, 0.65, 0.9];
/// Douglas-Peucker tolerances tried, as fractions of the hull perimeter.
const SIMPLIFY_STEPS: [f64; 6] = [0.005, 0.01, 0.02, 0.03, 0.045, 0.06];
/// Minimum |cos| between the gradient and a side's normal for a pixel to support that side.
const ALIGNMENT: f64 = 0.8;

/// A convex quadrilateral that may be a document outline.
#[derive(Clone, Debug, PartialEq)]
pub struct QuadCandidate {
    /// Corners in input-image coordinates: top-left, top-right, bottom-right, bottom-left
    /// (clockwise in image space, starting with the corner nearest the image's top-left).
    pub quad: Quad,
    /// Quadrilateral area divided by image area.
    pub area_fraction: f64,
    /// Long side over short side (averages of opposite sides), >= 1.
    pub aspect: f64,
    /// Fraction (0-1) of the four sides backed by an image edge: half the mean side support plus
    /// half the weakest side's.
    pub edge_support: f64,
    /// Area agreement (0-1) between the quadrilateral and the convex hull of its edge component.
    pub fit: f64,
    /// Document confidence (0-1); `DOCUMENT_DETECTED` when this candidate is the best one.
    pub confidence: f64,
}

/// Downscaled copy of the input used for edge analysis and face search.
pub(crate) struct Working {
    pub(crate) rgb: RgbImage,
    /// Working size over input size, per axis.
    pub(crate) sx: f64,
    pub(crate) sy: f64,
}

impl Working {
    pub(crate) fn new(image: &RgbImage) -> Working {
        let (w, h) = image.dimensions();
        let (ww, wh) = fit_long_side(w, h, WORKING_LONG_SIDE);
        Working {
            rgb: downscale_area(image, ww, wh),
            sx: f64::from(ww) / f64::from(w.max(1)),
            sy: f64::from(wh) / f64::from(h.max(1)),
        }
    }

    /// Working index coordinates (pixel centres at integers) to input continuous coordinates.
    fn to_input(&self, p: V2) -> V2 {
        V2::new((p.x + 0.5) / self.sx, (p.y + 0.5) / self.sy)
    }
}

/// All outline candidates in `image`, best (highest confidence) first.
pub fn find_quads(image: &RgbImage) -> Vec<QuadCandidate> {
    find_quads_in(&Working::new(image))
}

pub(crate) fn find_quads_in(working: &Working) -> Vec<QuadCandidate> {
    let gray = Gray::from_rgb(&working.rgb).blur(BLUR_SIGMA);
    let (w, h) = (gray.w, gray.h);
    if w < 16 || h < 16 {
        return Vec::new();
    }
    let grad = Gradients::sobel(&gray);
    let nms = grad.non_max_suppression();
    let mut strengths: Vec<f32> = nms
        .iter()
        .copied()
        .filter(|&m| m >= MIN_EDGE_STRENGTH)
        .collect();
    if strengths.len() < 64 {
        return Vec::new();
    }
    let image_area = (w * h) as f64;
    let min_area = MIN_AREA_FRACTION * image_area;
    let diagonal = f64::from(working.rgb.width()).hypot(f64::from(working.rgb.height()))
        / working.sx.min(working.sy);

    let mut out: Vec<QuadCandidate> = Vec::new();
    let mut last_high = f32::NAN;
    for level in LEVELS {
        let high = percentile(&mut strengths, level).max(2.0 * MIN_EDGE_STRENGTH);
        if high == last_high {
            continue;
        }
        last_high = high;
        let low = (0.5 * high).max(MIN_EDGE_STRENGTH);
        let edges = dilate(&hysteresis(&nms, w, h, low, high), w, h, 1);
        let (labels, blobs) = label_components(&edges, w, h);
        for blob in blobs.iter().filter(|b| b.bbox_area() as f64 >= min_area) {
            let hull = convex_hull(row_extremes(&labels, w, blob));
            let hull_area = signed_area(&hull);
            if hull_area < min_area {
                continue;
            }
            let Some(corners) = fit_quad(&hull) else {
                continue;
            };
            // Lines are fitted to this component's own edges, so print inside a document cannot
            // capture its sides.
            let component = Component {
                labels: &labels,
                width: w,
                height: h,
                label: blob.label,
            };
            let corners = refine(corners, &grad, &component, low);
            let area = signed_area(&corners);
            let fit = area.min(hull_area) / area.max(hull_area);
            let corners = extend_sides(corners, &grad);
            let Some(c) = score(working, &corners, fit, image_area, &grad) else {
                continue;
            };
            merge(&mut out, c, 0.015 * diagonal);
        }
    }
    out.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    out
}

/// Smallest aspect-match gain for which [`extend_sides`] moves a side.
const EXTENSION_MIN_GAIN: f64 = 0.25;
/// Long side of the copy on which the document path searches for outline candidates.
pub const SEARCH_LONG_SIDE: u32 = 640;
/// Half-width (working-image px) of the band in which a coarse side line is re-fitted.
const FINE_BAND: f64 = 3.0;
/// Candidates at or above this confidence after the coarse search are re-fitted and re-scored
/// at working resolution (all that can reach the detection threshold, with margin).
const FINE_MIN_CONFIDENCE: f64 = 0.3;
/// At most this many candidates are re-fitted.
const FINE_MAX: usize = 4;

/// Outline candidates for the document path, coarse to fine: the candidate search (edges at
/// several thresholds, components, hulls, quadrilateral fits) runs on a [`SEARCH_LONG_SIDE`]
/// copy; the side lines of the promising candidates are then re-fitted on the working copy's
/// gradient within a narrow band and the candidates re-scored there. Same accuracy as
/// [`find_quads_in`] at a fraction of the cost. `PROBE_DOCUMENT_PRESENT` keeps [`find_quads`].
pub(crate) fn find_document_quads(working: &Working) -> Vec<QuadCandidate> {
    let (ww, wh) = working.rgb.dimensions();
    let (cw, ch) = fit_long_side(ww, wh, SEARCH_LONG_SIDE);
    if (cw, ch) == (ww, wh) {
        return find_quads_in(working);
    }
    let coarse = Working {
        rgb: downscale_area(&working.rgb, cw, ch),
        sx: working.sx * f64::from(cw) / f64::from(ww),
        sy: working.sy * f64::from(ch) / f64::from(wh),
    };
    let mut candidates = find_quads_in(&coarse);
    if !candidates
        .iter()
        .any(|c| c.confidence >= FINE_MIN_CONFIDENCE)
    {
        return candidates;
    }
    let gray = Gray::from_rgb(&working.rgb).blur(BLUR_SIGMA);
    let grad = Gradients::sobel(&gray);
    let image_area = (gray.w * gray.h) as f64;
    for c in candidates
        .iter_mut()
        .filter(|c| c.confidence >= FINE_MIN_CONFIDENCE)
        .take(FINE_MAX)
    {
        // Input continuous coordinates to working index coordinates.
        let q = c.quad.0.map(|p| {
            V2::new(
                f64::from(p.x) * working.sx - 0.5,
                f64::from(p.y) * working.sy - 0.5,
            )
        });
        let band = |_: f64| FINE_BAND;
        let fine = refine_once(&q, &grad, None, MIN_EDGE_STRENGTH, &band)
            .and_then(|r| refine_once(&r, &grad, None, MIN_EDGE_STRENGTH, &band))
            .unwrap_or(q);
        if let Some(rescored) = score(working, &fine, c.fit, image_area, &grad) {
            *c = rescored;
        }
    }
    candidates.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    candidates
}

/// Approximate a convex hull by a quadrilateral: the tightest Douglas-Peucker tolerance that
/// leaves exactly four vertices.
fn fit_quad(hull: &[V2]) -> Option<[V2; 4]> {
    if hull.len() < 4 {
        return None;
    }
    let per = perimeter(hull);
    for step in SIMPLIFY_STEPS {
        let s = simplify_closed(hull, step * per);
        match s.len() {
            4 => {
                let q = [s[0], s[1], s[2], s[3]];
                return plausible(&q).then_some(q);
            }
            n if n < 4 => return None,
            _ => {}
        }
    }
    None
}

/// Convex, positively oriented, no tiny side, interior angles within 30-150 degrees.
fn plausible(q: &[V2; 4]) -> bool {
    if signed_area(q) <= 0.0 || !is_convex(q) {
        return false;
    }
    (0..4).all(|k| {
        let (a, b, c) = (q[k], q[(k + 1) % 4], q[(k + 2) % 4]);
        let (u, v) = (a - b, c - b);
        let (lu, lv) = (u.norm(), v.norm());
        if lu < 8.0 || lv < 8.0 {
            return false;
        }
        let cos = u.dot(v) / (lu * lv);
        cos.abs() <= 30f64.to_radians().cos()
    })
}

/// One connected component of the (dilated) edge map.
struct Component<'a> {
    labels: &'a [u32],
    width: usize,
    height: usize,
    label: u32,
}

impl Component<'_> {
    fn contains(&self, p: V2) -> bool {
        let (x, y) = (p.x.round(), p.y.round());
        x >= 0.0
            && y >= 0.0
            && (x as usize) < self.width
            && (y as usize) < self.height
            && self.labels[y as usize * self.width + x as usize] == self.label
    }
}

/// Snap the sides to the component's outer edges (two passes, narrowing the search band) and
/// intersect them for sub-pixel corners. Returns the input when refinement fails.
fn refine(
    corners: [V2; 4],
    grad: &Gradients,
    component: &Component<'_>,
    threshold: f32,
) -> [V2; 4] {
    let mut q = corners;
    for band in [0.03, 0.012] {
        let band = |len: f64| (band * len).clamp(3.0, 25.0);
        match refine_once(&q, grad, Some(component), threshold, &band) {
            Some(next) => q = next,
            None => break,
        }
    }
    q
}

/// `band` gives the search half-width (px) for a side of the given length.
fn refine_once(
    q: &[V2; 4],
    grad: &Gradients,
    component: Option<&Component<'_>>,
    threshold: f32,
    band: &dyn Fn(f64) -> f64,
) -> Option<[V2; 4]> {
    let mut lines = [Line {
        point: V2::default(),
        dir: V2::default(),
    }; 4];
    for (k, line) in lines.iter_mut().enumerate() {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        *line = fit_side(a, b, band((b - a).norm()), grad, component, threshold)
            .or_else(|| Line::through(a, b))?;
    }
    let mut out = [V2::default(); 4];
    for (k, corner) in out.iter_mut().enumerate() {
        *corner = lines[(k + 3) % 4].intersect(&lines[k])?;
    }
    let min_side = (0..4)
        .map(|k| (q[(k + 1) % 4] - q[k]).norm())
        .fold(f64::INFINITY, f64::min);
    let moved_too_far = out
        .iter()
        .zip(q)
        .any(|(n, o)| (*n - *o).norm() > 0.15 * min_side);
    (!moved_too_far && plausible(&out)).then_some(out)
}

/// Fit a line to the side from `a` to `b` (of a positively oriented polygon): on scanlines across
/// the middle 70% of the side, take the outermost strong edge (of `component`, when given) whose
/// gradient is aligned with the side's normal, refine it to sub-pixel along the normal, keep the
/// majority polarity and fit a robust line.
fn fit_side(
    a: V2,
    b: V2,
    band: f64,
    grad: &Gradients,
    component: Option<&Component<'_>>,
    threshold: f32,
) -> Option<Line> {
    let d = b - a;
    let len = d.norm();
    if len < 12.0 {
        return None;
    }
    let u = d * (1.0 / len);
    let outward = V2::new(u.y, -u.x);
    let magnitude = |p: V2| {
        let (gx, gy) = grad.sample(p.x, p.y);
        f64::from(gx.hypot(gy))
    };
    let reach = band.ceil() as i32;
    let steps = (0.7 * len).floor() as usize;
    let mut samples: Vec<(V2, bool)> = Vec::with_capacity(steps + 1);
    for s in 0..=steps {
        let base = a + u * (0.15 * len + s as f64);
        let mut hits: Vec<(i32, f64, bool)> = Vec::new();
        let mut strongest = 0.0f64;
        // Outside first: positive offsets point away from the quadrilateral.
        for off in (-reach..=reach).rev() {
            let p = base + outward * f64::from(off);
            if component.is_some_and(|c| !c.contains(p)) {
                continue;
            }
            let (gx, gy) = grad.sample(p.x, p.y);
            let m = f64::from(gx.hypot(gy));
            if m < f64::from(threshold) {
                continue;
            }
            let along = (f64::from(gx) * outward.x + f64::from(gy) * outward.y) / m;
            if along.abs() < ALIGNMENT {
                continue;
            }
            strongest = strongest.max(m);
            hits.push((off, m, along > 0.0));
        }
        let Some(&(off, _, polarity)) = hits.iter().find(|h| h.1 >= 0.6 * strongest) else {
            continue;
        };
        // Climb inward to the local maximum, then refine with a parabola.
        let mut o = f64::from(off);
        while o - 1.0 >= -f64::from(reach)
            && magnitude(base + outward * (o - 1.0)) > magnitude(base + outward * o)
        {
            o -= 1.0;
        }
        let (m_out, m0, m_in) = (
            magnitude(base + outward * (o + 1.0)),
            magnitude(base + outward * o),
            magnitude(base + outward * (o - 1.0)),
        );
        let denom = m_out - 2.0 * m0 + m_in;
        let delta = if denom < -1e-9 {
            (0.5 * (m_out - m_in) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        // The parabola's vertex is at `o - delta` along the outward axis.
        samples.push((base + outward * (o - delta), polarity));
    }
    let positive = samples.iter().filter(|s| s.1).count();
    let keep = positive * 2 >= samples.len();
    let pts: Vec<V2> = samples
        .iter()
        .filter(|s| s.1 == keep)
        .map(|s| s.0)
        .collect();
    if pts.len() < 6.max(steps * 3 / 10) {
        return None;
    }
    let (line, inliers) = Line::fit_robust(&pts)?;
    (inliers >= 6.max(steps / 4)).then_some(line)
}

/// Fraction of the side's middle 80% with an edge within 2 px that is aligned with the side and
/// has the side's majority polarity (a document border is darker or lighter than its
/// surroundings all along a side; texture is not).
fn side_support(a: V2, b: V2, grad: &Gradients, threshold: f32) -> f64 {
    let d = b - a;
    let len = d.norm();
    let steps = (0.8 * len).floor() as usize;
    if steps == 0 {
        return 0.0;
    }
    let u = d * (1.0 / len);
    let n = V2::new(u.y, -u.x);
    let (mut positive, mut negative) = (0usize, 0usize);
    for s in 0..steps {
        let base = a + u * (0.1 * len + s as f64 + 0.5);
        let mut best: Option<(f32, bool)> = None;
        for o in -2..=2 {
            let p = base + n * f64::from(o);
            let (gx, gy) = grad.sample(p.x, p.y);
            let m = gx.hypot(gy);
            if m < threshold || best.is_some_and(|(bm, _)| bm >= m) {
                continue;
            }
            let along = (f64::from(gx) * n.x + f64::from(gy) * n.y) / f64::from(m);
            if along.abs() >= ALIGNMENT {
                best = Some((m, along > 0.0));
            }
        }
        match best {
            Some((_, true)) => positive += 1,
            Some((_, false)) => negative += 1,
            None => {}
        }
    }
    positive.max(negative) as f64 / steps as f64
}

/// Edge support of a quadrilateral: half the mean side support plus half the weakest side's.
fn quad_support(corners: &[V2; 4], grad: &Gradients) -> f64 {
    let sides: Vec<f64> = (0..4)
        .map(|k| side_support(corners[k], corners[(k + 1) % 4], grad, MIN_EDGE_STRENGTH))
        .collect();
    let mean = sides.iter().sum::<f64>() / 4.0;
    let weakest = sides.iter().copied().fold(f64::INFINITY, f64::min);
    0.5 * mean + 0.5 * weakest
}

fn score(
    working: &Working,
    corners: &[V2; 4],
    fit: f64,
    image_area: f64,
    grad: &Gradients,
) -> Option<QuadCandidate> {
    let area = signed_area(corners);
    if area <= 0.0 {
        return None;
    }
    let input = order_corners(corners.map(|p| working.to_input(p)))?;
    let mut c = QuadCandidate {
        quad: Quad(input.map(V2::to_point)),
        area_fraction: area / image_area,
        aspect: aspect_of(&input),
        edge_support: quad_support(corners, grad),
        fit,
        confidence: 0.0,
    };
    c.confidence = document_confidence(&c);
    Some(c)
}

/// A band printed along a card edge in a colour close to the background (a dark header on a
/// dark table, blurred) can keep that edge out of the edge components, so the outline stops at
/// the band's inner edge. If moving one side outward onto a parallel, edge-supported line brings
/// the aspect ratio clearly closer to ID-1/TD3, take the best such move.
fn extend_sides(q: [V2; 4], grad: &Gradients) -> [V2; 4] {
    let targets = [ID1_ASPECT, TD3_ASPECT];
    let current = aspect_match(aspect_of(&q), &targets);
    // A move is taken only for a gain of at least 0.25 and matches never exceed 1, so a quad
    // already matching above 0.75 cannot change: skip the (costly) search.
    if current + EXTENSION_MIN_GAIN > 1.0 {
        return q;
    }
    let mut best: Option<([V2; 4], f64)> = None;
    for k in 0..4 {
        let (a, b) = (q[k], q[(k + 1) % 4]);
        let (Some(prev), Some(next)) = (
            Line::through(q[(k + 3) % 4], q[k]),
            Line::through(q[(k + 1) % 4], q[(k + 2) % 4]),
        ) else {
            continue;
        };
        let u = (b - a) * (1.0 / (b - a).norm().max(1e-9));
        let outward = V2::new(u.y, -u.x);
        let depth =
            0.5 * ((q[k] - q[(k + 3) % 4]).norm() + (q[(k + 2) % 4] - q[(k + 1) % 4]).norm());
        // Walk outward: first off the current edge (its blurred plateau), then keep the best
        // supported parallel line.
        let mut found: Option<(f64, f64)> = None;
        let mut off_current_edge = false;
        for off in 1..=(0.3 * depth).floor() as i32 {
            let shift = outward * f64::from(off);
            let s = side_support(a + shift, b + shift, grad, MIN_EDGE_STRENGTH);
            if !off_current_edge {
                off_current_edge = s < 0.5;
                continue;
            }
            if s >= 0.65 && found.is_none_or(|(_, best_s)| s > best_s) {
                found = Some((f64::from(off), s));
            }
        }
        let Some((off, _)) = found else { continue };
        let shift = outward * off;
        let Some(moved) = Line::through(a + shift, b + shift) else {
            continue;
        };
        // Snap the moved side to its edge, then rebuild its two corners.
        let (Some(c0), Some(c1)) = (prev.intersect(&moved), moved.intersect(&next)) else {
            continue;
        };
        let line = fit_side(c0, c1, 3.0, grad, None, MIN_EDGE_STRENGTH).unwrap_or(moved);
        let (Some(c0), Some(c1)) = (prev.intersect(&line), line.intersect(&next)) else {
            continue;
        };
        let mut candidate = q;
        candidate[k] = c0;
        candidate[(k + 1) % 4] = c1;
        let gain = aspect_match(aspect_of(&candidate), &targets);
        if plausible(&candidate)
            && gain >= 0.6
            && gain >= current + EXTENSION_MIN_GAIN
            && best.as_ref().is_none_or(|(_, g)| gain > *g)
        {
            best = Some((candidate, gain));
        }
    }
    best.map_or(q, |(candidate, _)| candidate)
}

/// Keep one candidate per outline: replace a near-duplicate (all corners within `tolerance`)
/// when the new one is more confident.
fn merge(out: &mut Vec<QuadCandidate>, c: QuadCandidate, tolerance: f64) {
    let same = |a: &QuadCandidate| {
        a.quad
            .0
            .iter()
            .zip(c.quad.0.iter())
            .all(|(p, q)| f64::from(p.distance(*q)) <= tolerance)
    };
    match out.iter_mut().find(|a| same(a)) {
        Some(existing) if existing.confidence < c.confidence => *existing = c,
        Some(_) => {}
        None => out.push(c),
    }
}

/// Order four corners clockwise on screen, starting with the corner nearest the image origin
/// (smallest x + y). `None` for degenerate quadrilaterals.
pub(crate) fn order_corners(mut q: [V2; 4]) -> Option<[V2; 4]> {
    let area = signed_area(&q);
    if !area.is_finite() || area.abs() < 1e-9 {
        return None;
    }
    if area < 0.0 {
        q.reverse();
    }
    let first = (0..4)
        .min_by(|&i, &j| (q[i].x + q[i].y).total_cmp(&(q[j].x + q[j].y)))
        .unwrap_or(0);
    q.rotate_left(first);
    Some(q)
}

/// Long over short side, using the averages of opposite sides.
pub(crate) fn aspect_of(q: &[V2; 4]) -> f64 {
    let side = |a: usize, b: usize| (q[b] - q[a]).norm();
    let w = 0.5 * (side(0, 1) + side(3, 2));
    let h = 0.5 * (side(0, 3) + side(1, 2));
    if w <= 0.0 || h <= 0.0 {
        return f64::INFINITY;
    }
    w.max(h) / w.min(h)
}

/// `(v - lo) / (hi - lo)`, clamped to [0, 1].
pub fn ramp(v: f64, lo: f64, hi: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// Match (0-1) of an aspect ratio to the nearest target: 1 within 3%, then a Gaussian fall-off
/// (sigma 8%) in log space.
pub fn aspect_match(aspect: f64, targets: &[f64]) -> f64 {
    targets
        .iter()
        .map(|t| {
            let d = (aspect / t).ln().abs();
            if d <= 0.03 {
                1.0
            } else {
                (-0.5 * ((d - 0.03) / 0.08).powi(2)).exp()
            }
        })
        .fold(0.0, f64::max)
}

/// Weighted geometric mean of scores in [0, 1]; zero if any score is zero.
pub fn weighted_geometric_mean(terms: &[(f64, f64)]) -> f64 {
    if terms.iter().any(|(v, _)| v.is_nan() || *v <= 0.0) {
        return 0.0;
    }
    let total: f64 = terms.iter().map(|(_, w)| w).sum();
    (terms.iter().map(|(v, w)| w * v.ln()).sum::<f64>() / total).exp()
}

/// Document confidence: ID-1/TD3 aspect, edge support, size and hull fit.
fn document_confidence(c: &QuadCandidate) -> f64 {
    let v = weighted_geometric_mean(&[
        (aspect_match(c.aspect, &[ID1_ASPECT, TD3_ASPECT]), 0.35),
        (ramp(c.edge_support, 0.3, 0.8), 0.35),
        (ramp(c.area_fraction, MIN_AREA_FRACTION, 0.12), 0.15),
        (ramp(c.fit, 0.8, 0.95), 0.15),
    ]);
    (v * 1e4).round() / 1e4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_ordered_clockwise_from_the_top_left() {
        let q = [
            V2::new(10.0, 90.0),
            V2::new(100.0, 95.0),
            V2::new(110.0, 5.0),
            V2::new(5.0, 10.0),
        ];
        let o = order_corners(q).unwrap();
        assert_eq!(o[0], V2::new(5.0, 10.0));
        assert_eq!(o[1], V2::new(110.0, 5.0));
        assert_eq!(o[2], V2::new(100.0, 95.0));
        assert!(signed_area(&o) > 0.0);
    }

    #[test]
    fn aspect_scores_prefer_id1_and_td3() {
        assert_eq!(aspect_match(1.586, &[ID1_ASPECT, TD3_ASPECT]), 1.0);
        assert_eq!(aspect_match(1.43, &[ID1_ASPECT, TD3_ASPECT]), 1.0);
        assert!(aspect_match(1.0, &[ID1_ASPECT, TD3_ASPECT]) < 0.01);
        assert!(aspect_match(2.2, &[ID1_ASPECT, TD3_ASPECT]) < 0.05);
        assert_eq!(weighted_geometric_mean(&[(1.0, 1.0), (0.0, 1.0)]), 0.0);
        assert!((weighted_geometric_mean(&[(0.25, 1.0), (1.0, 1.0)]) - 0.5).abs() < 1e-12);
    }
}
