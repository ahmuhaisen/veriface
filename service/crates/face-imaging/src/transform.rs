//! Resize, crop, letterbox, rotate, affine and projective warps.
//! Owner: vision agent.
//!
//! # Coordinate convention
//!
//! Continuous pixel coordinates follow OpenCV (and therefore MediaPipe's model preprocessing): the
//! centre of pixel (column `i`, row `j`) is at `(i, j)`, x to the right, y down.
//!
//! Warps compute every output pixel `(i, j)` by mapping the point `(i, j)` into the source and
//! sampling it with bilinear interpolation, exactly like `cv::warpAffine` / `cv::warpPerspective`
//! with `INTER_LINEAR`. The arithmetic is floating point (OpenCV quantises sub-pixel positions to
//! 1/32 px), and results are rounded to the nearest `u8`.
//!
//! Crops take whole pixels: [`crop`] copies columns `round(x) .. round(x + width)` and rows
//! `round(y) .. round(y + height)`, clamped to the image.

use image::RgbImage;

use crate::geom::{Affine, Point, Quad, Rect};

/// What a warp samples outside the source image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Border {
    /// A constant colour (`cv::BORDER_CONSTANT`). MediaPipe's `BORDER_ZERO` is `Constant([0, 0, 0])`.
    Constant([u8; 3]),
    /// The nearest edge pixel (`cv::BORDER_REPLICATE`).
    Replicate,
}

/// Resampling filter for [`resize`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Bilinear with pixel-centre alignment (`cv::resize` with `INTER_LINEAR`). Aliases when
    /// shrinking by more than 2x.
    Bilinear,
    /// Box filter with fractional pixel coverage when shrinking (`cv::resize` with `INTER_AREA`);
    /// bilinear on any axis that is enlarged. Use it for downscaling.
    Area,
}

// ------------------------------------------------------------------------------------- sampling

#[inline]
fn border_value(border: Border) -> [f32; 3] {
    match border {
        Border::Constant(c) => [f32::from(c[0]), f32::from(c[1]), f32::from(c[2])],
        Border::Replicate => [0.0; 3],
    }
}

#[inline]
fn fetch(raw: &[u8], w: usize, h: usize, x: i64, y: i64, border: Border) -> [f32; 3] {
    let (xi, yi) = if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
        (x as usize, y as usize)
    } else {
        match border {
            Border::Constant(_) => return border_value(border),
            Border::Replicate => (
                x.clamp(0, w as i64 - 1) as usize,
                y.clamp(0, h as i64 - 1) as usize,
            ),
        }
    };
    let i = (yi * w + xi) * 3;
    [
        f32::from(raw[i]),
        f32::from(raw[i + 1]),
        f32::from(raw[i + 2]),
    ]
}

/// Bilinear sample of `src` at continuous coordinates `(x, y)` (pixel centres at integers), per
/// channel, in 0..=255 (not rounded). Non-finite coordinates give the border colour (black for
/// [`Border::Replicate`]).
pub fn sample_bilinear(src: &RgbImage, x: f32, y: f32, border: Border) -> [f32; 3] {
    let (w, h) = (src.width() as usize, src.height() as usize);
    sample_raw(src.as_raw(), w, h, x, y, border)
}

#[inline]
fn sample_raw(raw: &[u8], w: usize, h: usize, x: f32, y: f32, border: Border) -> [f32; 3] {
    if w == 0 || h == 0 || !x.is_finite() || !y.is_finite() {
        return border_value(border);
    }
    // Clamp far-away coordinates so the i64 conversion cannot overflow; replicate and constant
    // borders give the same result there as at the clamp position.
    let limit = (w.max(h) as f32) + 4.0;
    let (x, y) = (x.clamp(-limit, limit), y.clamp(-limit, limit));
    let (xf, yf) = (x.floor(), y.floor());
    let (fx, fy) = (x - xf, y - yf);
    let (x0, y0) = (xf as i64, yf as i64);
    let p00 = fetch(raw, w, h, x0, y0, border);
    let p10 = fetch(raw, w, h, x0 + 1, y0, border);
    let p01 = fetch(raw, w, h, x0, y0 + 1, border);
    let p11 = fetch(raw, w, h, x0 + 1, y0 + 1, border);
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let top = p00[c] + (p10[c] - p00[c]) * fx;
        let bottom = p01[c] + (p11[c] - p01[c]) * fx;
        out[c] = top + (bottom - top) * fy;
    }
    out
}

#[inline]
fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

// ---------------------------------------------------------------------------------------- warps

/// Warp with an affine map from **output** to **source** coordinates: output pixel `(i, j)` is the
/// bilinear sample of `src` at `dst_to_src.apply((i, j))`.
pub fn warp_affine_inverse(
    src: &RgbImage,
    dst_to_src: &Affine,
    width: u32,
    height: u32,
    border: Border,
) -> RgbImage {
    let mut out = RgbImage::new(width, height);
    if width == 0 || height == 0 {
        return out;
    }
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let raw = src.as_raw();
    let [a, b, c, d, e, f] = dst_to_src.0;
    for (j, row) in out.chunks_exact_mut(width as usize * 3).enumerate() {
        let jf = j as f32;
        for (i, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let fi = i as f32;
            let x = a * fi + b * jf + c;
            let y = d * fi + e * jf + f;
            let v = sample_raw(raw, sw, sh, x, y, border);
            px[0] = to_u8(v[0]);
            px[1] = to_u8(v[1]);
            px[2] = to_u8(v[2]);
        }
    }
    out
}

/// Like `cv::warpAffine(src, m, (width, height), INTER_LINEAR, border)`: `src_to_dst` maps source
/// coordinates to output coordinates. Returns `None` when the map is singular.
///
/// Typical use: face alignment to the 112x112 ArcFace template with the transform from
/// [`estimate_similarity`].
pub fn warp_affine(
    src: &RgbImage,
    src_to_dst: &Affine,
    width: u32,
    height: u32,
    border: Border,
) -> Option<RgbImage> {
    let inv = src_to_dst.invert()?;
    Some(warp_affine_inverse(src, &inv, width, height, border))
}

/// A 3x3 projective transform (row-major) acting on `(x, y, 1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    pub const IDENTITY: Homography = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    /// The homography of an affine transform.
    pub fn from_affine(a: &Affine) -> Homography {
        let [a0, a1, a2, a3, a4, a5] = a.0.map(f64::from);
        Homography([a0, a1, a2, a3, a4, a5, 0.0, 0.0, 1.0])
    }

    /// Apply to a point; `None` when the point maps to infinity.
    pub fn apply(&self, p: Point) -> Option<Point> {
        let m = &self.0;
        let (x, y) = (f64::from(p.x), f64::from(p.y));
        let w = m[6] * x + m[7] * y + m[8];
        if w.abs() < 1e-12 {
            return None;
        }
        Some(Point::new(
            ((m[0] * x + m[1] * y + m[2]) / w) as f32,
            ((m[3] * x + m[4] * y + m[5]) / w) as f32,
        ))
    }

    /// Inverse, or `None` when singular.
    pub fn invert(&self) -> Option<Homography> {
        let m = &self.0;
        let c00 = m[4] * m[8] - m[5] * m[7];
        let c01 = m[5] * m[6] - m[3] * m[8];
        let c02 = m[3] * m[7] - m[4] * m[6];
        let det = m[0] * c00 + m[1] * c01 + m[2] * c02;
        if !det.is_finite() || det.abs() < 1e-15 {
            return None;
        }
        let inv = 1.0 / det;
        Some(Homography([
            c00 * inv,
            (m[2] * m[7] - m[1] * m[8]) * inv,
            (m[1] * m[5] - m[2] * m[4]) * inv,
            c01 * inv,
            (m[0] * m[8] - m[2] * m[6]) * inv,
            (m[2] * m[3] - m[0] * m[5]) * inv,
            c02 * inv,
            (m[1] * m[6] - m[0] * m[7]) * inv,
            (m[0] * m[4] - m[1] * m[3]) * inv,
        ]))
    }

    /// `self` after `first`: applying the result equals applying `first`, then `self`.
    pub fn after(&self, first: &Homography) -> Homography {
        let (a, b) = (&self.0, &first.0);
        let mut r = [0.0; 9];
        for i in 0..3 {
            for j in 0..3 {
                r[i * 3 + j] = (0..3).map(|k| a[i * 3 + k] * b[k * 3 + j]).sum();
            }
        }
        Homography(r)
    }

    /// The homography mapping each `from[k]` onto `to[k]` (exact for four points in general
    /// position, like `cv::getPerspectiveTransform`). `None` when degenerate.
    pub fn from_correspondences(from: &[Point; 4], to: &[Point; 4]) -> Option<Homography> {
        // 8x8 system for h0..h7 with h8 = 1; column 8 is the right-hand side.
        let mut a = [[0.0f64; 9]; 8];
        for k in 0..4 {
            let (x, y) = (f64::from(from[k].x), f64::from(from[k].y));
            let (u, v) = (f64::from(to[k].x), f64::from(to[k].y));
            a[2 * k] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
            a[2 * k + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
        }
        let h = solve_8x8(a)?;
        Some(Homography([
            h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0,
        ]))
    }

    /// Maps the corners of `quad` (top-left, top-right, bottom-right, bottom-left) onto the corners
    /// of a `width` x `height` output, corner to corner: `(0, 0)`, `(width, 0)`,
    /// `(width, height)`, `(0, height)` (the same corner semantics as MediaPipe's ROI warps).
    pub fn quad_to_rect(quad: &Quad, width: f32, height: f32) -> Option<Homography> {
        let to = [
            Point::new(0.0, 0.0),
            Point::new(width, 0.0),
            Point::new(width, height),
            Point::new(0.0, height),
        ];
        Homography::from_correspondences(&quad.0, &to)
    }
}

/// Gaussian elimination with partial pivoting on an 8x9 augmented matrix.
fn solve_8x8(mut m: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for col in 0..8 {
        let pivot = (col..8).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[pivot][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, pivot);
        let pivot_row = m[col];
        for (r, row) in m.iter_mut().enumerate() {
            if r == col {
                continue;
            }
            let factor = row[col] / pivot_row[col];
            if factor != 0.0 {
                for k in col..9 {
                    row[k] -= factor * pivot_row[k];
                }
            }
        }
    }
    let mut x = [0.0; 8];
    for (i, xi) in x.iter_mut().enumerate() {
        *xi = m[i][8] / m[i][i];
        if !xi.is_finite() {
            return None;
        }
    }
    Some(x)
}

/// Warp with a projective map from **output** to **source** coordinates (see
/// [`warp_affine_inverse`]).
pub fn warp_perspective_inverse(
    src: &RgbImage,
    dst_to_src: &Homography,
    width: u32,
    height: u32,
    border: Border,
) -> RgbImage {
    let mut out = RgbImage::new(width, height);
    if width == 0 || height == 0 {
        return out;
    }
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let raw = src.as_raw();
    let m = &dst_to_src.0;
    for (j, row) in out.chunks_exact_mut(width as usize * 3).enumerate() {
        let jf = j as f64;
        for (i, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let fi = i as f64;
            let w = m[6] * fi + m[7] * jf + m[8];
            let (x, y) = if w.abs() < 1e-12 {
                (f32::NAN, f32::NAN)
            } else {
                (
                    ((m[0] * fi + m[1] * jf + m[2]) / w) as f32,
                    ((m[3] * fi + m[4] * jf + m[5]) / w) as f32,
                )
            };
            let v = sample_raw(raw, sw, sh, x, y, border);
            px[0] = to_u8(v[0]);
            px[1] = to_u8(v[1]);
            px[2] = to_u8(v[2]);
        }
    }
    out
}

/// Like `cv::warpPerspective(src, m, (width, height), INTER_LINEAR, border)`: `src_to_dst` maps
/// source to output coordinates. `None` when singular.
pub fn warp_perspective(
    src: &RgbImage,
    src_to_dst: &Homography,
    width: u32,
    height: u32,
    border: Border,
) -> Option<RgbImage> {
    let inv = src_to_dst.invert()?;
    Some(warp_perspective_inverse(src, &inv, width, height, border))
}

// ------------------------------------------------------------------------------ rotated rects

/// A rotated rectangle, as MediaPipe's `RotatedRect`: centre and size in source pixels, and a
/// rotation in radians, clockwise in the image (y points down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotatedRect {
    pub center: Point,
    pub width: f32,
    pub height: f32,
    pub rotation: f32,
}

impl RotatedRect {
    /// An axis-aligned rectangle.
    pub fn from_rect(r: &Rect) -> RotatedRect {
        RotatedRect {
            center: r.center(),
            width: r.width,
            height: r.height,
            rotation: 0.0,
        }
    }

    /// The rectangle's own x axis (unit vector) in image coordinates.
    pub fn axis_x(&self) -> Point {
        Point::new(self.rotation.cos(), self.rotation.sin())
    }

    /// The rectangle's own y axis (unit vector) in image coordinates.
    pub fn axis_y(&self) -> Point {
        Point::new(-self.rotation.sin(), self.rotation.cos())
    }

    /// Corners: top-left, top-right, bottom-right, bottom-left (in the rectangle's own frame).
    pub fn corners(&self) -> [Point; 4] {
        let (u, v) = (self.axis_x(), self.axis_y());
        let (hw, hh) = (self.width * 0.5, self.height * 0.5);
        let c = self.center;
        [
            c - u * hw - v * hh,
            c + u * hw - v * hh,
            c + u * hw + v * hh,
            c - u * hw + v * hh,
        ]
    }

    /// Axis-aligned bounding box.
    pub fn bounding_box(&self) -> Rect {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in self.corners() {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// The map from an `out_width` x `out_height` output to source coordinates used by MediaPipe's
    /// `ImageToTensorCalculator` (OpenCV path): the rectangle's corners map to the output corners
    /// `(0, 0)` .. `(out_width, out_height)`, corner to corner. It is also the map MediaPipe's
    /// `LandmarkProjectionCalculator` uses to bring model outputs back into the image.
    pub fn output_to_source(&self, out_width: u32, out_height: u32) -> Affine {
        let (cos, sin) = (self.rotation.cos(), self.rotation.sin());
        let sx = self.width / out_width as f32;
        let sy = self.height / out_height as f32;
        let (hw, hh) = (self.width * 0.5, self.height * 0.5);
        Affine([
            cos * sx,
            -sin * sy,
            self.center.x - cos * hw + sin * hh,
            sin * sx,
            cos * sy,
            self.center.y - sin * hw - cos * hh,
        ])
    }

    /// Enlarge (never shrink) to the aspect ratio `out_width : out_height`, padding both sides
    /// equally. Returns the padded rectangle and the padding fractions `[left, top, right, bottom]`
    /// of the output (MediaPipe `PadRoi` with `keep_aspect_ratio = true`).
    pub fn pad_to_aspect(&self, out_width: u32, out_height: u32) -> (RotatedRect, [f32; 4]) {
        let tensor_aspect = out_height as f32 / out_width as f32;
        let roi_aspect = self.height / self.width;
        let mut r = *self;
        if tensor_aspect > roi_aspect {
            r.height = self.width * tensor_aspect;
            let v = (1.0 - roi_aspect / tensor_aspect) / 2.0;
            (r, [0.0, v, 0.0, v])
        } else {
            r.width = self.height / tensor_aspect;
            let hpad = (1.0 - tensor_aspect / roi_aspect) / 2.0;
            (r, [hpad, 0.0, hpad, 0.0])
        }
    }
}

/// Extract a rotated rectangle into an `out_width` x `out_height` image with MediaPipe's corner
/// mapping (see [`RotatedRect::output_to_source`]).
pub fn crop_rotated(
    src: &RgbImage,
    roi: &RotatedRect,
    out_width: u32,
    out_height: u32,
    border: Border,
) -> RgbImage {
    warp_affine_inverse(
        src,
        &roi.output_to_source(out_width, out_height),
        out_width,
        out_height,
        border,
    )
}

// ----------------------------------------------------------------------------------- letterbox

/// Aspect-preserving fit of a source image into an output, centred, as MediaPipe does for the face
/// detector (`keep_aspect_ratio = true`, ROI = whole image).
///
/// The map is `source = (output - pad) / scale`, applied to output pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Letterbox {
    /// Output pixels per source pixel.
    pub scale: f32,
    /// Horizontal offset, in output pixels.
    pub pad_x: f32,
    /// Vertical offset, in output pixels.
    pub pad_y: f32,
    pub width: u32,
    pub height: u32,
}

impl Letterbox {
    /// Fit `src_width` x `src_height` into `width` x `height`. MediaPipe's whole-image ROI is
    /// centred at `(src_width / 2, src_height / 2)`; this reproduces its mapping exactly.
    pub fn fit(src_width: u32, src_height: u32, width: u32, height: u32) -> Letterbox {
        let roi = RotatedRect {
            center: Point::new(src_width as f32 * 0.5, src_height as f32 * 0.5),
            width: src_width as f32,
            height: src_height as f32,
            rotation: 0.0,
        };
        let (padded, _) = roi.pad_to_aspect(width, height);
        let scale = width as f32 / padded.width;
        Letterbox {
            scale,
            pad_x: (padded.width - src_width as f32) * 0.5 * scale,
            pad_y: (padded.height - src_height as f32) * 0.5 * scale,
            width,
            height,
        }
    }

    pub fn to_source(&self, p: Point) -> Point {
        Point::new(
            (p.x - self.pad_x) / self.scale,
            (p.y - self.pad_y) / self.scale,
        )
    }

    pub fn to_output(&self, p: Point) -> Point {
        Point::new(p.x * self.scale + self.pad_x, p.y * self.scale + self.pad_y)
    }

    pub fn output_to_source(&self) -> Affine {
        let k = 1.0 / self.scale;
        Affine([k, 0.0, -self.pad_x * k, 0.0, k, -self.pad_y * k])
    }

    pub fn source_to_output(&self) -> Affine {
        Affine([self.scale, 0.0, self.pad_x, 0.0, self.scale, self.pad_y])
    }
}

/// Letterbox `src` into `width` x `height` with bilinear sampling and `fill` padding (MediaPipe's
/// detector input when `fill` is black).
pub fn letterbox(src: &RgbImage, width: u32, height: u32, fill: [u8; 3]) -> (RgbImage, Letterbox) {
    let lb = Letterbox::fit(src.width(), src.height(), width, height);
    let img = warp_affine_inverse(
        src,
        &lb.output_to_source(),
        width,
        height,
        Border::Constant(fill),
    );
    (img, lb)
}

// ---------------------------------------------------------------------------------------- crop

/// A crop and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Cropped {
    pub image: RgbImage,
    /// Column of the crop's first pixel in the source.
    pub x: u32,
    /// Row of the crop's first pixel in the source.
    pub y: u32,
}

impl Cropped {
    /// Source coordinates of a point given in crop coordinates.
    pub fn to_source(&self, p: Point) -> Point {
        Point::new(p.x + self.x as f32, p.y + self.y as f32)
    }

    /// Crop coordinates of a point given in source coordinates.
    pub fn to_crop(&self, p: Point) -> Point {
        Point::new(p.x - self.x as f32, p.y - self.y as f32)
    }
}

/// Copy the whole pixels covered by `rect` (columns `round(x) .. round(x + width)`, rows likewise),
/// clamped to the image. `None` when nothing is left.
pub fn crop(src: &RgbImage, rect: &Rect) -> Option<Cropped> {
    let (w, h) = (src.width() as f32, src.height() as f32);
    let finite = [rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|v| v.is_finite());
    if !finite {
        return None;
    }
    let x0 = rect.x.round().clamp(0.0, w);
    let y0 = rect.y.round().clamp(0.0, h);
    let x1 = (rect.x + rect.width).round().clamp(0.0, w);
    let y1 = (rect.y + rect.height).round().clamp(0.0, h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (x, y) = (x0 as u32, y0 as u32);
    let image = crop_pixels(src, x, y, (x1 - x0) as u32, (y1 - y0) as u32)?;
    Some(Cropped { image, x, y })
}

/// A whole-pixel rectangle: columns `x .. x + width`, rows `y .. y + height`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl PixelRect {
    /// The square of side `side` centred on `center`, clamped to a `width` x `height` image with
    /// `x0 = max(0, floor(cx - side/2))`, `x1 = min(width, ceil(cx + side/2))` (rows likewise).
    /// Clamping shrinks the square, so the result may be non-square; nothing is padded. This is
    /// the golden rule for the segmentation ROI and the landmark fallback crop
    /// (`testdata/golden/README.md`). `None` when nothing of it is inside the image.
    pub fn square_clamped(center: Point, side: f32, width: u32, height: u32) -> Option<PixelRect> {
        if !(center.x.is_finite() && center.y.is_finite() && side.is_finite() && side > 0.0) {
            return None;
        }
        let half = f64::from(side) / 2.0;
        let (cx, cy) = (f64::from(center.x), f64::from(center.y));
        let x0 = (cx - half).floor().max(0.0);
        let y0 = (cy - half).floor().max(0.0);
        let x1 = (cx + half).ceil().min(f64::from(width));
        let y1 = (cy + half).ceil().min(f64::from(height));
        (x1 > x0 && y1 > y0).then_some(PixelRect {
            x: x0 as u32,
            y: y0 as u32,
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
        })
    }

    pub fn to_rect(&self) -> Rect {
        Rect::new(
            self.x as f32,
            self.y as f32,
            self.width as f32,
            self.height as f32,
        )
    }

    /// The same area as a [`RotatedRect`] (rotation 0), as MediaPipe sees a crop it is given as
    /// a whole image: centre `(x + width/2, y + height/2)`.
    pub fn to_rotated(&self) -> RotatedRect {
        RotatedRect::from_rect(&self.to_rect())
    }

    /// Copy these pixels out of `src`. `None` when the rectangle does not fit inside it.
    pub fn crop(&self, src: &RgbImage) -> Option<RgbImage> {
        crop_pixels(src, self.x, self.y, self.width, self.height)
    }
}

/// Copy an exact pixel rectangle. `None` when it does not fit inside the image or is empty.
pub fn crop_pixels(src: &RgbImage, x: u32, y: u32, width: u32, height: u32) -> Option<RgbImage> {
    if width == 0
        || height == 0
        || x.checked_add(width)? > src.width()
        || y.checked_add(height)? > src.height()
    {
        return None;
    }
    let sw = src.width() as usize;
    let raw = src.as_raw();
    let mut data = Vec::with_capacity(width as usize * height as usize * 3);
    for row in y as usize..(y + height) as usize {
        let start = (row * sw + x as usize) * 3;
        data.extend_from_slice(&raw[start..start + width as usize * 3]);
    }
    RgbImage::from_raw(width, height, data)
}

// -------------------------------------------------------------------------------------- rotate

/// Clockwise rotation by a multiple of 90 degrees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rotation {
    None,
    Cw90,
    Cw180,
    Cw270,
}

impl Rotation {
    pub const ALL: [Rotation; 4] = [
        Rotation::None,
        Rotation::Cw90,
        Rotation::Cw180,
        Rotation::Cw270,
    ];

    /// Clockwise degrees: 0, 90, 180 or 270.
    pub fn degrees(self) -> u32 {
        match self {
            Rotation::None => 0,
            Rotation::Cw90 => 90,
            Rotation::Cw180 => 180,
            Rotation::Cw270 => 270,
        }
    }

    /// From clockwise degrees (any multiple of 90, including negative ones).
    pub fn from_degrees(degrees: i32) -> Option<Rotation> {
        if degrees % 90 != 0 {
            return None;
        }
        Some(match degrees.rem_euclid(360) {
            0 => Rotation::None,
            90 => Rotation::Cw90,
            180 => Rotation::Cw180,
            _ => Rotation::Cw270,
        })
    }

    /// The rotation that undoes this one.
    pub fn inverse(self) -> Rotation {
        match self {
            Rotation::Cw90 => Rotation::Cw270,
            Rotation::Cw270 => Rotation::Cw90,
            r => r,
        }
    }

    /// Size of a `width` x `height` image after rotation.
    pub fn rotated_size(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Rotation::None | Rotation::Cw180 => (width, height),
            Rotation::Cw90 | Rotation::Cw270 => (height, width),
        }
    }

    /// Maps a point of a `width` x `height` source into the rotated image (pixel centres at
    /// integers, so pixel `(i, j)` maps exactly onto the pixel [`rotate`] moves it to).
    pub fn forward(self, width: u32, height: u32) -> Affine {
        let (w1, h1) = (width as f32 - 1.0, height as f32 - 1.0);
        match self {
            Rotation::None => Affine::IDENTITY,
            Rotation::Cw90 => Affine([0.0, -1.0, h1, 1.0, 0.0, 0.0]),
            Rotation::Cw180 => Affine([-1.0, 0.0, w1, 0.0, -1.0, h1]),
            Rotation::Cw270 => Affine([0.0, 1.0, 0.0, -1.0, 0.0, w1]),
        }
    }

    /// Maps a point of the rotated image back into the `width` x `height` source.
    pub fn backward(self, width: u32, height: u32) -> Affine {
        let (rw, rh) = self.rotated_size(width, height);
        self.inverse().forward(rw, rh)
    }
}

/// Rotate clockwise by a multiple of 90 degrees (exact pixel permutation).
pub fn rotate(src: &RgbImage, rotation: Rotation) -> RgbImage {
    match rotation {
        Rotation::None => src.clone(),
        Rotation::Cw90 => image::imageops::rotate90(src),
        Rotation::Cw180 => image::imageops::rotate180(src),
        Rotation::Cw270 => image::imageops::rotate270(src),
    }
}

// -------------------------------------------------------------------------------------- resize

/// Per-output-index source contributions along one axis: first source index and weights.
struct AxisWeights {
    start: Vec<usize>,
    weights: Vec<Vec<f32>>,
}

fn axis_weights(src: usize, dst: usize, filter: Filter) -> AxisWeights {
    let scale = src as f64 / dst as f64;
    let mut start = Vec::with_capacity(dst);
    let mut weights = Vec::with_capacity(dst);
    if filter == Filter::Area && scale > 1.0 {
        for d in 0..dst {
            let lo = d as f64 * scale;
            let hi = ((d + 1) as f64 * scale).min(src as f64);
            let first = lo.floor() as usize;
            let last = (hi.ceil() as usize).min(src).max(first + 1);
            let w = (first..last)
                .map(|i| {
                    let overlap = (hi.min((i + 1) as f64) - lo.max(i as f64)).max(0.0);
                    (overlap / (hi - lo)) as f32
                })
                .collect();
            start.push(first);
            weights.push(w);
        }
    } else {
        for d in 0..dst {
            let s = ((d as f64 + 0.5) * scale - 0.5).clamp(0.0, (src - 1) as f64);
            let i0 = s.floor() as usize;
            let i1 = (i0 + 1).min(src - 1);
            let f = (s - i0 as f64) as f32;
            start.push(i0);
            weights.push(if i1 == i0 {
                vec![1.0]
            } else {
                vec![1.0 - f, f]
            });
        }
    }
    AxisWeights { start, weights }
}

/// Resize to exactly `width` x `height`.
pub fn resize(src: &RgbImage, width: u32, height: u32, filter: Filter) -> RgbImage {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let (dw, dh) = (width as usize, height as usize);
    if dw == 0 || dh == 0 || sw == 0 || sh == 0 {
        return RgbImage::new(width, height);
    }
    if sw == dw && sh == dh {
        return src.clone();
    }
    let xw = axis_weights(sw, dw, filter);
    let yw = axis_weights(sh, dh, filter);
    let raw = src.as_raw();

    // Horizontal pass into f32 rows (sh x dw x 3).
    let mut tmp = vec![0.0f32; sh * dw * 3];
    for (src_row, dst_row) in raw.chunks_exact(sw * 3).zip(tmp.chunks_exact_mut(dw * 3)) {
        for (x, out) in dst_row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let s0 = xw.start[x];
            let mut acc = [0.0f32; 3];
            for (k, wk) in xw.weights[x].iter().enumerate() {
                let i = (s0 + k) * 3;
                acc[0] += f32::from(src_row[i]) * wk;
                acc[1] += f32::from(src_row[i + 1]) * wk;
                acc[2] += f32::from(src_row[i + 2]) * wk;
            }
            out.copy_from_slice(&acc);
        }
    }

    // Vertical pass.
    let mut out = RgbImage::new(width, height);
    let mut acc = vec![0.0f32; dw * 3];
    for (y, out_row) in out.chunks_exact_mut(dw * 3).enumerate() {
        acc.iter_mut().for_each(|v| *v = 0.0);
        let s0 = yw.start[y];
        for (k, wk) in yw.weights[y].iter().enumerate() {
            let row = &tmp[(s0 + k) * dw * 3..(s0 + k + 1) * dw * 3];
            for (a, v) in acc.iter_mut().zip(row) {
                *a += v * wk;
            }
        }
        for (o, a) in out_row.iter_mut().zip(&acc) {
            *o = to_u8(*a);
        }
    }
    out
}

/// Resize by a uniform factor (output size rounded, at least 1x1). [`Filter::Area`] is the right
/// choice when `factor < 1`.
pub fn resize_by(src: &RgbImage, factor: f32, filter: Filter) -> RgbImage {
    let w = ((src.width() as f32 * factor).round() as u32).max(1);
    let h = ((src.height() as f32 * factor).round() as u32).max(1);
    resize(src, w, h, filter)
}

// ------------------------------------------------------------------------- similarity estimate

/// Least-squares similarity transform (rotation, uniform scale, translation; never a reflection)
/// mapping `src[k]` onto `dst[k]`. This is the Umeyama estimate that
/// `skimage.transform.SimilarityTransform.estimate` (InsightFace `norm_crop`) computes. `None` with
/// fewer than two point pairs, mismatched lengths or coincident source points.
pub fn estimate_similarity(src: &[Point], dst: &[Point]) -> Option<Affine> {
    if src.len() != dst.len() || src.len() < 2 {
        return None;
    }
    let n = src.len() as f64;
    let (mut sx, mut sy, mut dx, mut dy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (s, d) in src.iter().zip(dst) {
        sx += f64::from(s.x);
        sy += f64::from(s.y);
        dx += f64::from(d.x);
        dy += f64::from(d.y);
    }
    let (sx, sy, dx, dy) = (sx / n, sy / n, dx / n, dy / n);
    let (mut num_a, mut num_b, mut den) = (0.0f64, 0.0f64, 0.0f64);
    for (s, d) in src.iter().zip(dst) {
        let (px, py) = (f64::from(s.x) - sx, f64::from(s.y) - sy);
        let (qx, qy) = (f64::from(d.x) - dx, f64::from(d.y) - dy);
        num_a += px * qx + py * qy;
        num_b += px * qy - py * qx;
        den += px * px + py * py;
    }
    if den < 1e-12 {
        return None;
    }
    let (a, b) = (num_a / den, num_b / den);
    let tx = dx - (a * sx - b * sy);
    let ty = dy - (b * sx + a * sy);
    Some(Affine([
        a as f32, -b as f32, tx as f32, b as f32, a as f32, ty as f32,
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn gradient(w: u32, h: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| {
            Rgb([
                (x * 7 % 256) as u8,
                (y * 5 % 256) as u8,
                ((x + y) % 256) as u8,
            ])
        })
    }

    fn close(a: Point, b: Point, tol: f32) -> bool {
        (a - b).norm() <= tol
    }

    #[test]
    fn identity_warp_is_exact_and_constant_border_fills_outside() {
        let src = gradient(17, 11);
        let out = warp_affine_inverse(&src, &Affine::IDENTITY, 17, 11, Border::Replicate);
        assert_eq!(out, src);
        // Shift right by 3: the first three columns come from outside the source.
        let shifted = warp_affine_inverse(
            &src,
            &Affine([1.0, 0.0, -3.0, 0.0, 1.0, 0.0]),
            17,
            11,
            Border::Constant([9, 8, 7]),
        );
        assert_eq!(shifted.get_pixel(0, 0), &Rgb([9, 8, 7]));
        assert_eq!(shifted.get_pixel(3, 5), src.get_pixel(0, 5));
        assert_eq!(shifted.get_pixel(16, 10), src.get_pixel(13, 10));
    }

    #[test]
    fn bilinear_sample_interpolates_and_replicates() {
        let src = RgbImage::from_fn(2, 1, |x, _| Rgb([if x == 0 { 0 } else { 100 }, 0, 0]));
        assert!((sample_bilinear(&src, 0.25, 0.0, Border::Replicate)[0] - 25.0).abs() < 1e-4);
        assert!((sample_bilinear(&src, -5.0, 0.0, Border::Replicate)[0]).abs() < 1e-4);
        assert!((sample_bilinear(&src, 7.0, 3.0, Border::Replicate)[0] - 100.0).abs() < 1e-4);
        assert!((sample_bilinear(&src, 1e30, 0.0, Border::Replicate)[0] - 100.0).abs() < 1e-4);
        // Half a pixel outside with a constant border blends with the border colour.
        let v = sample_bilinear(&src, 1.5, 0.0, Border::Constant([0, 0, 0]))[0];
        assert!((v - 50.0).abs() < 1e-4);
        assert_eq!(
            sample_bilinear(&src, f32::NAN, 0.0, Border::Constant([7, 7, 7])),
            [7.0; 3]
        );
    }

    #[test]
    fn warp_affine_matches_its_inverse_form() {
        let src = gradient(40, 30);
        let m = Affine([0.9, -0.2, 3.0, 0.2, 0.9, -1.0]);
        let a = warp_affine(&src, &m, 32, 24, Border::Replicate).unwrap();
        let b = warp_affine_inverse(&src, &m.invert().unwrap(), 32, 24, Border::Replicate);
        assert_eq!(a, b);
        assert!(warp_affine(&src, &Affine([0.0; 6]), 4, 4, Border::Replicate).is_none());
    }

    #[test]
    fn similarity_recovers_a_known_transform_without_reflection() {
        let truth = Affine([0.8, -0.6, 10.0, 0.6, 0.8, -5.0]);
        let src = [
            Point::new(38.29, 51.69),
            Point::new(73.53, 51.50),
            Point::new(56.02, 71.73),
            Point::new(41.54, 92.36),
            Point::new(70.72, 92.20),
        ];
        let dst = src.map(|p| truth.apply(p));
        let est = estimate_similarity(&src, &dst).unwrap();
        for (e, t) in est.0.iter().zip(truth.0) {
            assert!((e - t).abs() < 1e-4, "{est:?}");
        }
        // A mirrored target cannot be matched by a similarity: the estimate stays a rotation.
        let mirrored: Vec<Point> = src.iter().map(|p| Point::new(-p.x, p.y)).collect();
        let m = estimate_similarity(&src, &mirrored).unwrap();
        assert!(m.0[0] * m.0[4] - m.0[1] * m.0[3] >= 0.0);
        assert!(estimate_similarity(&src[..1], &dst[..1]).is_none());
        assert!(estimate_similarity(&[Point::new(1.0, 1.0); 3], &dst[..3]).is_none());
    }

    #[test]
    fn rotated_rect_corner_mapping_matches_mediapipe() {
        // Axis-aligned: output (0,0) is the top-left corner, (W,H) the bottom-right corner.
        let r = RotatedRect {
            center: Point::new(50.0, 40.0),
            width: 20.0,
            height: 10.0,
            rotation: 0.0,
        };
        let m = r.output_to_source(4, 2);
        assert!(close(
            m.apply(Point::new(0.0, 0.0)),
            Point::new(40.0, 35.0),
            1e-5
        ));
        assert!(close(
            m.apply(Point::new(4.0, 2.0)),
            Point::new(60.0, 45.0),
            1e-5
        ));
        // Rotated by +90 degrees (clockwise): the rectangle's x axis points down the image.
        let r = RotatedRect {
            rotation: std::f32::consts::FRAC_PI_2,
            ..r
        };
        let m = r.output_to_source(4, 2);
        let corners = r.corners();
        assert!(close(m.apply(Point::new(0.0, 0.0)), corners[0], 1e-4));
        assert!(close(m.apply(Point::new(4.0, 0.0)), corners[1], 1e-4));
        assert!(close(m.apply(Point::new(4.0, 2.0)), corners[2], 1e-4));
        assert!(close(corners[1], Point::new(55.0, 50.0), 1e-4));
        let bb = r.bounding_box();
        assert!((bb.width - 10.0).abs() < 1e-4 && (bb.height - 20.0).abs() < 1e-4);
    }

    #[test]
    fn letterbox_reproduces_mediapipe_padding() {
        // 200x100 into 128x128: scale 0.64, vertical padding 32 px on each side.
        let lb = Letterbox::fit(200, 100, 128, 128);
        assert!((lb.scale - 0.64).abs() < 1e-6);
        assert!(lb.pad_x.abs() < 1e-5);
        assert!((lb.pad_y - 32.0).abs() < 1e-4);
        let p = Point::new(123.0, 45.0);
        assert!(close(lb.to_source(lb.to_output(p)), p, 1e-4));
        assert!(close(lb.output_to_source().apply(lb.to_output(p)), p, 1e-4));
        assert!(close(lb.source_to_output().apply(p), lb.to_output(p), 1e-4));
        let (img, _) = letterbox(&gradient(200, 100), 128, 128, [0, 0, 0]);
        assert_eq!(img.get_pixel(64, 0), &Rgb([0, 0, 0]));
        assert_ne!(img.get_pixel(64, 64), &Rgb([0, 0, 0]));
        // Padding fractions as PadRoi reports them.
        let roi = RotatedRect {
            center: Point::new(100.0, 50.0),
            width: 200.0,
            height: 100.0,
            rotation: 0.0,
        };
        let (padded, pad) = roi.pad_to_aspect(128, 128);
        assert_eq!(padded.height, 200.0);
        assert!((pad[1] - 0.25).abs() < 1e-6 && pad[0] == 0.0);
    }

    #[test]
    fn rotation_point_maps_agree_with_pixel_rotation() {
        let src = gradient(7, 4);
        for rot in Rotation::ALL {
            let out = rotate(&src, rot);
            assert_eq!(
                (out.width(), out.height()),
                rot.rotated_size(src.width(), src.height())
            );
            let fwd = rot.forward(7, 4);
            let back = rot.backward(7, 4);
            for (x, y) in [(0u32, 0u32), (6, 0), (2, 3), (6, 3)] {
                let p = fwd.apply(Point::new(x as f32, y as f32));
                let (px, py) = (p.x.round() as u32, p.y.round() as u32);
                assert_eq!(
                    out.get_pixel(px, py),
                    src.get_pixel(x, y),
                    "{rot:?} ({x},{y})"
                );
                assert!(close(back.apply(p), Point::new(x as f32, y as f32), 1e-5));
            }
        }
        assert_eq!(Rotation::from_degrees(-90), Some(Rotation::Cw270));
        assert_eq!(Rotation::from_degrees(45), None);
        assert_eq!(Rotation::Cw90.inverse(), Rotation::Cw270);
    }

    #[test]
    fn crop_clamps_and_reports_its_origin() {
        let src = gradient(20, 10);
        let c = crop(&src, &Rect::new(-5.0, 2.2, 10.0, 100.0)).unwrap();
        assert_eq!((c.x, c.y, c.image.width(), c.image.height()), (0, 2, 5, 8));
        assert_eq!(c.image.get_pixel(1, 1), src.get_pixel(1, 3));
        assert_eq!(
            c.to_source(c.to_crop(Point::new(3.0, 4.0))),
            Point::new(3.0, 4.0)
        );
        assert!(crop(&src, &Rect::new(30.0, 0.0, 5.0, 5.0)).is_none());
        assert!(crop(&src, &Rect::new(f32::NAN, 0.0, 5.0, 5.0)).is_none());
        assert!(crop_pixels(&src, 18, 0, 3, 1).is_none());
    }

    #[test]
    fn square_clamped_follows_the_golden_rule() {
        // Inside: floor / ceil of the exact square.
        let r = PixelRect::square_clamped(Point::new(50.3, 40.0), 10.0, 200, 100).unwrap();
        assert_eq!(
            r,
            PixelRect {
                x: 45,
                y: 35,
                width: 11,
                height: 10
            }
        );
        // Clamping shrinks; nothing is padded.
        let r = PixelRect::square_clamped(Point::new(5.0, 95.0), 40.0, 200, 100).unwrap();
        assert_eq!(
            r,
            PixelRect {
                x: 0,
                y: 75,
                width: 25,
                height: 25
            }
        );
        assert!(PixelRect::square_clamped(Point::new(-50.0, 5.0), 10.0, 20, 20).is_none());
        assert!(PixelRect::square_clamped(Point::new(5.0, 5.0), f32::NAN, 20, 20).is_none());
        let src = gradient(20, 10);
        let c = PixelRect {
            x: 2,
            y: 3,
            width: 4,
            height: 2,
        }
        .crop(&src)
        .unwrap();
        assert_eq!(c.get_pixel(0, 0), src.get_pixel(2, 3));
        assert_eq!(
            PixelRect {
                x: 2,
                y: 3,
                width: 4,
                height: 2
            }
            .to_rotated()
            .center,
            Point::new(4.0, 4.0)
        );
    }

    #[test]
    fn area_resize_averages_blocks_and_both_filters_preserve_constants() {
        // 4x4 checkerboard of 2x2 blocks averaged down to 2x2 gives exact block means.
        let src = RgbImage::from_fn(4, 4, |x, y| {
            let v = if (x / 2 + y / 2) % 2 == 0 { 200 } else { 100 };
            Rgb([v, v, v])
        });
        let out = resize(&src, 2, 2, Filter::Area);
        assert_eq!(out.get_pixel(0, 0), &Rgb([200, 200, 200]));
        assert_eq!(out.get_pixel(1, 0), &Rgb([100, 100, 100]));
        // Fractional coverage: 3 -> 2 pixels. Pixel 0 covers [0, 1.5): (0 + 90 * 0.5) / 1.5 = 30.
        let row = RgbImage::from_fn(3, 1, |x, _| Rgb([[0u8, 90, 180][x as usize], 0, 0]));
        let out = resize(&row, 2, 1, Filter::Area);
        assert_eq!(out.get_pixel(0, 0)[0], 30);
        assert_eq!(out.get_pixel(1, 0)[0], 150);
        let flat = RgbImage::from_pixel(13, 7, Rgb([42, 43, 44]));
        for f in [Filter::Area, Filter::Bilinear] {
            for (w, h) in [(5, 3), (26, 14), (13, 7)] {
                assert!(
                    resize(&flat, w, h, f)
                        .pixels()
                        .all(|p| *p == Rgb([42, 43, 44]))
                );
            }
        }
        assert_eq!(resize_by(&flat, 0.5, Filter::Area).dimensions(), (7, 4));
    }

    #[test]
    fn homography_maps_quad_corners_and_inverts() {
        let quad = Quad([
            Point::new(10.0, 12.0),
            Point::new(110.0, 5.0),
            Point::new(120.0, 80.0),
            Point::new(5.0, 70.0),
        ]);
        let h = Homography::quad_to_rect(&quad, 86.0, 54.0).unwrap();
        let want = [(0.0, 0.0), (86.0, 0.0), (86.0, 54.0), (0.0, 54.0)];
        for (p, (x, y)) in quad.0.iter().zip(want) {
            assert!(close(h.apply(*p).unwrap(), Point::new(x, y), 1e-3));
        }
        let inv = h.invert().unwrap();
        let p = Point::new(40.0, 30.0);
        assert!(close(inv.apply(h.apply(p).unwrap()).unwrap(), p, 1e-3));
        let id = h.after(&inv);
        for (a, b) in id.0.iter().zip(Homography::IDENTITY.0) {
            assert!((a / id.0[8] - b).abs() < 1e-9);
        }
        // An affine homography warps like warp_affine.
        let src = gradient(30, 20);
        let m = Affine([1.1, 0.1, -2.0, -0.1, 0.9, 3.0]);
        let a = warp_affine(&src, &m, 25, 18, Border::Replicate).unwrap();
        let b = warp_perspective(
            &src,
            &Homography::from_affine(&m),
            25,
            18,
            Border::Replicate,
        )
        .unwrap();
        let max_diff = a
            .as_raw()
            .iter()
            .zip(b.as_raw())
            .map(|(x, y)| (i16::from(*x) - i16::from(*y)).abs())
            .max()
            .unwrap();
        assert!(max_diff <= 1, "{max_diff}");
    }
}
