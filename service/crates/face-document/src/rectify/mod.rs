//! Document outline detection and perspective rectification (docs/architecture.md section 3.2,
//! step 2). Classical, pure-Rust computer vision:
//!
//! 1. Downscale to [`WORKING_LONG_SIDE`] (area average), convert to luma, Gaussian blur.
//! 2. Canny-style edges at three adaptive thresholds; close 1-px gaps; connected components.
//! 3. For each large component: convex hull, Douglas-Peucker simplification to a convex
//!    quadrilateral, then sub-pixel side lines fitted to the strongest outer gradient and
//!    intersected (this recovers the virtual sharp corners of rounded cards).
//! 4. Score each quadrilateral ([`QuadCandidate`]): aspect ratio against ID-1 (1.586) and TD3
//!    (~1.42), edge support along the four sides, size, and agreement with the hull. The best
//!    confidence is `DOCUMENT_DETECTED`.
//! 5. [`rectified_frame`] maps the outline to a rectangle with a homography sized so that no
//!    direction is upscaled; pixels are always sampled from the full-resolution input.

mod edges;
mod projective;
mod quad;
mod raster;
pub(crate) mod shapes;
mod warp;

use face_imaging::RgbImage;
use face_imaging::geom::{Quad, Rect};

pub use projective::Projective;
pub use quad::{
    ID1_ASPECT, MIN_AREA_FRACTION, QuadCandidate, TD3_ASPECT, WORKING_LONG_SIDE, find_quads,
};
pub use warp::warp_projective;

#[cfg(test)]
pub(crate) use quad::find_quads_in;
pub(crate) use quad::{Working, find_document_quads};
pub(crate) use warp::{downscale_area, fit_long_side, fit_within};

pub use quad::{aspect_match, ramp, weighted_geometric_mean};

use shapes::{V2, clipped_area, signed_area};

/// Confidence at or above which a document outline is used for rectification. Internal
/// algorithm setting (the profile's `DOCUMENT_DETECTED` range is a separate, policy setting).
pub const DETECTION_THRESHOLD: f64 = 0.5;

/// The best document outline in an image.
#[derive(Clone, Debug, PartialEq)]
pub struct DocumentDetection {
    /// Confidence (0-1) of the best candidate, 0 when there is none. Reported as
    /// `DOCUMENT_DETECTED`.
    pub confidence: f64,
    /// The best candidate, whatever its confidence.
    pub candidate: Option<QuadCandidate>,
}

impl DocumentDetection {
    /// True when the best candidate reaches [`DETECTION_THRESHOLD`].
    pub fn detected(&self) -> bool {
        self.candidate.is_some() && self.confidence >= DETECTION_THRESHOLD
    }

    fn from_candidates(candidates: Vec<QuadCandidate>) -> DocumentDetection {
        let candidate = candidates.into_iter().next();
        DocumentDetection {
            confidence: candidate.as_ref().map_or(0.0, |c| c.confidence),
            candidate,
        }
    }
}

/// Detect the document outline in `image`.
pub fn detect_document(image: &RgbImage) -> DocumentDetection {
    DocumentDetection::from_candidates(find_quads(image))
}

/// A rectified document: a `width` x `height` rectangle in full-resolution pixels and the
/// homography that maps it onto the outline in the input image (continuous coordinates).
#[derive(Clone, Debug, PartialEq)]
pub struct RectifiedFrame {
    pub width: u32,
    pub height: u32,
    pub to_input: Projective,
}

/// The rectangle for `quad` (corners top-left, top-right, bottom-right, bottom-left in input
/// coordinates). Its aspect ratio is the average of opposite sides; its size is the largest at
/// which no direction anywhere in the document is upscaled. `None` when degenerate or smaller
/// than 16 px.
pub fn rectified_frame(quad: &Quad) -> Option<RectifiedFrame> {
    let c = quad.0.map(V2::from_point);
    let side = |a: usize, b: usize| (c[b] - c[a]).norm();
    let w0 = 0.5 * (side(0, 1) + side(3, 2));
    let h0 = 0.5 * (side(0, 3) + side(1, 2));
    if !(w0.is_finite() && h0.is_finite()) || w0 < 8.0 || h0 < 8.0 {
        return None;
    }
    let rect = |w: f64, h: f64| [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
    let dst = c.map(|p| (p.x, p.y));
    let unit = Projective::from_points_f64(&rect(w0, h0), &dst)?;
    let mut stretch = f64::INFINITY;
    for gy in 0..=8 {
        for gx in 0..=8 {
            let (x, y) = (w0 * f64::from(gx) / 8.0, h0 * f64::from(gy) / 8.0);
            stretch = stretch.min(unit.min_stretch(x, y)?);
        }
    }
    let k = stretch.min(1.0);
    let (width, height) = ((w0 * k).floor(), (h0 * k).floor());
    if width < 16.0 || height < 16.0 {
        return None;
    }
    let to_input = Projective::from_points_f64(&rect(width, height), &dst)?;
    Some(RectifiedFrame {
        width: width as u32,
        height: height as u32,
        to_input,
    })
}

/// Warp the whole document to its [`rectified_frame`], sampling the full-resolution `image`.
/// The result keeps the image-space orientation of `quad` (its top-left corner at the origin).
pub fn rectify(image: &RgbImage, quad: &Quad) -> Option<RgbImage> {
    let frame = rectified_frame(quad)?;
    Some(warp_projective(
        image,
        &frame.to_input,
        frame.width,
        frame.height,
    ))
}

/// Area of a quadrilateral.
pub fn quad_area(quad: &Quad) -> f64 {
    signed_area(&quad.0.map(V2::from_point)).abs()
}

/// Fraction (0-1) of `rect`'s area that lies inside the convex quadrilateral `quad`.
pub fn coverage(quad: &Quad, rect: &Rect) -> f64 {
    let area = f64::from(rect.area());
    if area <= 0.0 {
        return 0.0;
    }
    let mut clip = quad.0.map(V2::from_point);
    if signed_area(&clip) < 0.0 {
        clip.reverse();
    }
    let r = [
        V2::new(f64::from(rect.x), f64::from(rect.y)),
        V2::new(f64::from(rect.right()), f64::from(rect.y)),
        V2::new(f64::from(rect.right()), f64::from(rect.bottom())),
        V2::new(f64::from(rect.x), f64::from(rect.bottom())),
    ];
    (clipped_area(&r, &clip) / area).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests;
