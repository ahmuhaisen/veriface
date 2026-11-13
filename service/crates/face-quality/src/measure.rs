//! Pixel and landmark measures behind the selfie / portrait checks.
//!
//! Each measure returns a value in the check's unit (`contracts/checks.yaml`) or the reason it
//! could not be assessed (`Err`, which becomes `undetermined`). Definitions:
//!
//! - **Regions.** *Face region*: the polygon of the 36 face-oval landmarks. *Face skin*
//!   (`definitions.faceSkinRegion`): face region pixels the segmenter labels face-skin; without
//!   segmentation, the face region minus eyes, brows and lips. *Occlusion region*: the face region with the top clipped halfway
//!   between the brow tops and the hairline (landmark 10), shrunk to 90 % about its centroid, so
//!   that hairline and contour misalignment do not count.
//! - **Work image.** Pixel measures run on the head region resampled (area filter) so that the
//!   inter-eye distance is at most [`WORK_IOD`]; smaller faces are not upscaled. The illumination
//!   measures run on the head region at full image resolution.
//! - **Luma** (`definitions.luma`) is OFIQ's luminance: sRGB-linearised channels,
//!   `Y = 0.2126 R + 0.7152 G + 0.0722 B`, stored as `floor(255 Y + 0.5)`. `LUMINANCE_MEAN` = mean
//!   face-skin luma / 255. `UNDER_EXPOSURE` = share of face skin with luma <= 25 and
//!   `OVER_EXPOSURE` = share with luma >= 247. `DYNAMIC_RANGE` = entropy of the face-skin luma
//!   histogram, bits. `ILLUMINATION_UNIFORMITY` = darkest / brightest mean luma of four squares of
//!   side `0.3 IOD` centred on the forehead (landmark 151), chin (199) and cheeks (205, 425), over
//!   their skin pixels (face- or body-skin inside the face region; at least a quarter of each).
//!   Texture measures (sharpness, background) use gamma-encoded BT.601 `Y'` (`stats::luma`).
//! - **Sharpness** = `1 - blur`, blur being the Crete-Roffet perceptual blur of the face region
//!   (luma), after resampling the face to an inter-eye distance of [`SHARPNESS_IOD`] px.
//! - **Background uniformity** = `exp(-g / 8) * exp(-s / 64)` over segmenter background pixels of
//!   the segmentation ROI at [`SHARPNESS_IOD`] scale (eroded 2 px), `g` = mean luma gradient
//!   (grey levels per pixel), `s` = luma standard deviation.
//! - **Eyes visible** = for each eye, the face-skin share of an ellipse over the eye opening
//!   (corner to corner, height 0.6 x eye width); the worse eye.
//! - **Eyes open** = `1 - max(eyeBlinkLeft, eyeBlinkRight)`; without blendshapes, the eye aspect
//!   ratio of the less open eye / 0.28, clamped.
//! - **Mouth** = `max(jawOpen, clamp(2 * inner-lip gap / mouth width))`.
//! - **Expression** = the strongest of the smile, frown, raised-brow, nose-sneer, pucker and
//!   cheek-puff blendshapes (`browDown` is excluded: it reads 0.5-0.8 on calm faces).
//! - **Face occlusion** = share of the occlusion region labelled body-skin, clothes, others, or
//!   hair above the nose tip outside the eyebrows (hair below the nose is facial hair);
//!   background pixels are ignored.
//! - **Head covering** = share of others / clothes in the head-top region (crown to hairline,
//!   80 % of the head width), background ignored.
//! - **Eyeglasses** (heuristic) = `clamp(others share in the spectacle band / 0.08)`, raised by a
//!   horizontal-edge signal at the nose bridge.
//! - **Gaze** = angle between the camera axis and the gaze direction: head pose plus the iris
//!   offset within each eye (eyeball radius 12 mm, eye half-width 15 mm, half-opening 5 mm).

use face_imaging::RgbImage;
use face_imaging::geom::Point;
use face_imaging::stats::{self, Histogram, Mask, Plane};
use face_imaging::transform::{self, Filter};
use face_models::{Blendshape, BlendshapeScores, Point3, SegmentClass, Segmentation, mesh};

use crate::geometry::{FaceFrame, HeadGeometry};
use crate::pose::Pose;

/// A measured value, or why the measure could not be assessed.
pub type Measure = Result<f64, String>;

/// Largest inter-eye distance (px) of the work image used by the pixel measures.
pub const WORK_IOD: f32 = 128.0;
/// Inter-eye distance (px) the face is resampled to for SHARPNESS and BACKGROUND_UNIFORMITY.
pub const SHARPNESS_IOD: f32 = 64.0;
/// `definitions.underExposureLevel`: linear luma <= 25 is under-exposed.
pub const UNDER_EXPOSURE_MAX: u8 = 25;
/// `definitions.overExposureLevel`: linear luma >= 247 is over-exposed.
pub const OVER_EXPOSURE_MIN: u8 = 247;
/// Fewest pixels a region must have for a pixel measure.
const MIN_PIXELS: usize = 50;
/// Class id used for "outside the segmentation".
const UNKNOWN: u8 = 255;

/// Everything the measures need about one face.
pub struct FaceContext<'a> {
    pub image: &'a RgbImage,
    /// 478 landmarks in image coordinates.
    pub landmarks: &'a [Point3],
    pub frame: FaceFrame,
    pub head: HeadGeometry,
    pub blendshapes: Option<&'a BlendshapeScores>,
    pub pose: Option<Pose>,
    pub segmentation: Option<&'a Segmentation>,
}

impl FaceContext<'_> {
    fn lm(&self, i: usize) -> Point {
        self.landmarks[i].xy()
    }
}

/// The head region resampled for pixel measures.
pub struct Work {
    pub image: RgbImage,
    /// Image coordinates of the crop's first pixel.
    origin: (f32, f32),
    /// Work pixels per image pixel (horizontal; the vertical scale differs only by rounding).
    pub scale: f32,
    scale_y: f32,
    /// Segment class per work pixel (`UNKNOWN` outside the segmentation); `None` without one.
    classes: Option<Vec<u8>>,
}

impl Work {
    /// Crop the head box (+10 % per side) and downscale it to [`WORK_IOD`].
    pub fn new(ctx: &FaceContext<'_>) -> Option<Work> {
        Work::with_max_iod(ctx, WORK_IOD)
    }

    /// The head region at full image resolution (for the photometric measures, which the contract
    /// and the goldens define on input-image pixels).
    pub fn full_resolution(ctx: &FaceContext<'_>) -> Option<Work> {
        Work::with_max_iod(ctx, f32::INFINITY)
    }

    fn with_max_iod(ctx: &FaceContext<'_>, max_iod: f32) -> Option<Work> {
        let b = ctx.head.head_box;
        let rect = face_imaging::geom::Rect::new(
            b.x - 0.1 * b.width,
            b.y - 0.1 * b.height,
            1.2 * b.width,
            1.2 * b.height,
        );
        let crop = transform::crop(ctx.image, &rect)?;
        let target = (max_iod / ctx.frame.iod).min(1.0);
        let (cw, ch) = crop.image.dimensions();
        let image = if target < 1.0 {
            transform::resize_by(&crop.image, target, Filter::Area)
        } else {
            crop.image
        };
        let mut work = Work {
            scale: image.width() as f32 / cw as f32,
            scale_y: image.height() as f32 / ch as f32,
            image,
            origin: (crop.x as f32, crop.y as f32),
            classes: None,
        };
        if let Some(seg) = ctx.segmentation {
            let (w, h) = work.image.dimensions();
            let mut classes = Vec::with_capacity(w as usize * h as usize);
            for j in 0..h {
                for i in 0..w {
                    let p = work.to_image(Point::new(i as f32, j as f32));
                    classes.push(seg.class_at(p).map_or(UNKNOWN, |c| c as u8));
                }
            }
            work.classes = Some(classes);
        }
        Some(work)
    }

    pub fn width(&self) -> u32 {
        self.image.width()
    }

    pub fn height(&self) -> u32 {
        self.image.height()
    }

    /// Image -> work coordinates (pixel centres preserved under the area resampling).
    pub fn to_work(&self, p: Point) -> Point {
        Point::new(
            (p.x - self.origin.0 + 0.5) * self.scale - 0.5,
            (p.y - self.origin.1 + 0.5) * self.scale_y - 0.5,
        )
    }

    /// Work -> image coordinates.
    pub fn to_image(&self, p: Point) -> Point {
        Point::new(
            (p.x + 0.5) / self.scale - 0.5 + self.origin.0,
            (p.y + 0.5) / self.scale_y - 0.5 + self.origin.1,
        )
    }

    fn polygon(&self, ctx: &FaceContext<'_>, idx: &[usize]) -> Vec<Point> {
        idx.iter().map(|&i| self.to_work(ctx.lm(i))).collect()
    }

    fn poly_mask(&self, poly: &[Point]) -> Mask {
        Mask::from_polygon(self.width(), self.height(), poly)
    }

    fn class(&self, i: usize) -> Option<SegmentClass> {
        let c = self.classes.as_ref()?[i];
        (c != UNKNOWN).then(|| SegmentClass::from_id(c))
    }

    fn class_mask(&self, pred: impl Fn(SegmentClass) -> bool) -> Option<Mask> {
        let classes = self.classes.as_ref()?;
        let w = self.width() as usize;
        Some(Mask::from_fn(self.width(), self.height(), |x, y| {
            let c = classes[y as usize * w + x as usize];
            c != UNKNOWN && pred(SegmentClass::from_id(c))
        }))
    }

    /// Eye-opening ellipse of one eye: corners `outer`, `inner`.
    fn eye_ellipse(&self, ctx: &FaceContext<'_>, outer: usize, inner: usize) -> Mask {
        let (a, b) = (self.to_work(ctx.lm(outer)), self.to_work(ctx.lm(inner)));
        let d = b - a;
        let len = d.norm();
        Mask::from_ellipse(
            self.width(),
            self.height(),
            a.midpoint(b),
            0.5 * len,
            0.3 * len,
            d.y.atan2(d.x),
        )
    }

    fn eyes(&self, ctx: &FaceContext<'_>) -> Mask {
        self.eye_ellipse(ctx, 33, 133)
            .or(&self.eye_ellipse(ctx, 263, 362))
    }

    /// Face features (eyes, brows, lips) for the no-segmentation fallback.
    fn features(&self, ctx: &FaceContext<'_>) -> Mask {
        let mut m = self.eyes(ctx);
        for idx in [
            &mesh::RIGHT_EYEBROW[..],
            &mesh::LEFT_EYEBROW[..],
            &mesh::LIPS_OUTER[..],
        ] {
            m = m.or(&self.poly_mask(&self.polygon(ctx, idx)));
        }
        m.dilate(1)
    }

    fn face_region(&self, ctx: &FaceContext<'_>) -> Mask {
        self.poly_mask(&self.polygon(ctx, &mesh::FACE_OVAL))
    }

    fn skin(&self, ctx: &FaceContext<'_>) -> Mask {
        let face = self.face_region(ctx);
        match self.class_mask(|c| c == SegmentClass::FaceSkin) {
            Some(skin) => face.and(&skin),
            None => face.and_not(&self.features(ctx)),
        }
    }
}

fn shrink(poly: &[Point], factor: f32) -> Vec<Point> {
    let n = poly.len().max(1) as f32;
    let c = poly.iter().fold(Point::default(), |a, p| a + *p) * (1.0 / n);
    poly.iter().map(|p| c + (*p - c) * factor).collect()
}

// ------------------------------------------------------------------------------- geometry

/// Ratios from the head geometry, for a `width` x `height` image.
pub struct GeometryRatios {
    pub head_in_frame: f64,
    pub eye_mid_horizontal: f64,
    pub eye_mid_vertical: f64,
    pub head_width: f64,
    pub head_height: f64,
}

pub fn geometry_ratios(head: &HeadGeometry, width: u32, height: u32) -> GeometryRatios {
    GeometryRatios {
        head_in_frame: f64::from(head.in_frame_ratio(width, height)),
        eye_mid_horizontal: f64::from(head.eye_mid.x / width as f32),
        eye_mid_vertical: f64::from(head.eye_mid.y / height as f32),
        head_width: f64::from(head.width / width as f32),
        head_height: f64::from(head.height / height as f32),
    }
}

// ---------------------------------------------------------------------------- illumination

/// The five illumination measures.
pub struct Illumination {
    pub luminance_mean: Measure,
    pub under_exposure: Measure,
    pub over_exposure: Measure,
    pub dynamic_range: Measure,
    pub uniformity: Measure,
}

pub fn illumination(ctx: &FaceContext<'_>, work: &Work) -> Illumination {
    let (w, h) = work.image.dimensions();
    let lum8 = stats::luminance8(&work.image);
    let skin = work.skin(ctx);
    let face = work.face_region(ctx);
    let skin_ok = skin.count() >= MIN_PIXELS.max(face.count() / 10);
    let hist = Histogram::from_u8(&lum8, w, Some(&skin));
    let need_skin = |f: &dyn Fn(&Histogram) -> Option<f64>| -> Measure {
        if !skin_ok {
            return Err(format!(
                "too little visible face skin ({} px at work scale)",
                skin.count()
            ));
        }
        f(&hist).ok_or_else(|| "empty face-skin histogram".to_owned())
    };
    let luminance_mean = need_skin(&|h| h.mean().map(|m| m / 255.0));
    let under_exposure = need_skin(&|h| h.fraction_in(0, UNDER_EXPOSURE_MAX));
    let dynamic_range = need_skin(&|h| h.entropy_bits());

    let over_exposure = need_skin(&|h| h.fraction_in(OVER_EXPOSURE_MIN, 255));

    let uniformity = (|| -> Measure {
        let lum8u = &lum8;
        // Zones take any skin the segmenter sees (it sometimes labels the chin body-skin).
        let valid = match work
            .class_mask(|c| matches!(c, SegmentClass::FaceSkin | SegmentClass::BodySkin))
        {
            Some(any_skin) => face.and(&any_skin),
            None => face.and_not(&work.features(ctx)),
        };
        let side = (0.3 * ctx.frame.iod * work.scale).max(3.0);
        let mut means = Vec::with_capacity(4);
        for (name, idx) in [
            ("forehead", 151usize),
            ("chin", 199),
            ("right cheek", mesh::RIGHT_CHEEK),
            ("left cheek", mesh::LEFT_CHEEK),
        ] {
            let c = work.to_work(ctx.lm(idx));
            let (x0, y0) = ((c.x - side / 2.0).round(), (c.y - side / 2.0).round());
            let (x1, y1) = ((c.x + side / 2.0).round(), (c.y + side / 2.0).round());
            let (mut sum, mut n, mut total) = (0.0f64, 0usize, 0usize);
            for y in y0.max(0.0) as u32..(y1.max(0.0) as u32).min(h) {
                for x in x0.max(0.0) as u32..(x1.max(0.0) as u32).min(w) {
                    total += 1;
                    if valid.get(x, y) {
                        sum += f64::from(lum8u[(y * w + x) as usize]);
                        n += 1;
                    }
                }
            }
            let zone_px = ((x1 - x0) * (y1 - y0)).max(1.0) as usize;
            if total * 2 < zone_px || n * 4 < zone_px || n == 0 {
                return Err(format!("the {name} zone is not visible face skin"));
            }
            means.push(sum / n as f64);
        }
        let (lo, hi) = means
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), m| (lo.min(*m), hi.max(*m)));
        if hi <= 1e-6 {
            return Err("face zones are black".to_owned());
        }
        Ok(lo / hi)
    })();

    Illumination {
        luminance_mean,
        under_exposure,
        over_exposure,
        dynamic_range,
        uniformity,
    }
}

// ------------------------------------------------------------------------------- sharpness

pub fn sharpness(ctx: &FaceContext<'_>, work: &Work) -> Measure {
    let factor = SHARPNESS_IOD / (ctx.frame.iod * work.scale);
    let poly = shrink(&work.polygon(ctx, &mesh::FACE_OVAL), 0.9);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &poly {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    let rect = face_imaging::geom::Rect::new(x0 - 4.0, y0 - 4.0, x1 - x0 + 8.0, y1 - y0 + 8.0);
    let crop = transform::crop(&work.image, &rect).ok_or("face region outside the image")?;
    let filter = if factor < 1.0 {
        Filter::Area
    } else {
        Filter::Bilinear
    };
    let resized = transform::resize_by(&crop.image, factor, filter);
    let (rw, rh) = resized.dimensions();
    let fx = rw as f32 / crop.image.width() as f32;
    let fy = rh as f32 / crop.image.height() as f32;
    let poly: Vec<Point> = poly
        .iter()
        .map(|p| {
            Point::new(
                (p.x - crop.x as f32 + 0.5) * fx - 0.5,
                (p.y - crop.y as f32 + 0.5) * fy - 0.5,
            )
        })
        .collect();
    let mask = Mask::from_polygon(rw, rh, &poly).erode(1);
    if mask.count() < 4 * MIN_PIXELS {
        return Err("face region too small for sharpness".to_owned());
    }
    let blur = stats::blur_crete_roffet(&stats::luma(&resized), Some(&mask), 9)
        .ok_or("the face region has no texture")?;
    Ok((1.0 - blur).clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------- background

pub fn background_uniformity(ctx: &FaceContext<'_>) -> Measure {
    let seg = ctx.segmentation.ok_or("no segmentation")?;
    let roi = seg.roi;
    let crop = roi
        .crop(ctx.image)
        .ok_or("segmentation ROI outside the image")?;
    let g = SHARPNESS_IOD / ctx.frame.iod;
    let filter = if g < 1.0 {
        Filter::Area
    } else {
        Filter::Bilinear
    };
    let resized = transform::resize_by(&crop, g, filter);
    let (rw, rh) = resized.dimensions();
    let (sx, sy) = (roi.width as f32 / rw as f32, roi.height as f32 / rh as f32);
    let mask = Mask::from_fn(rw, rh, |x, y| {
        let i = (((x as f32 + 0.5) * sx - 0.5).round().max(0.0) as u32).min(roi.width - 1);
        let j = (((y as f32 + 0.5) * sy - 0.5).round().max(0.0) as u32).min(roi.height - 1);
        seg.class(i, j) == SegmentClass::Background
    })
    .erode(2);
    if mask.count() < 4 * MIN_PIXELS {
        return Err(format!(
            "too little background ({} px at {SHARPNESS_IOD} px IOD)",
            mask.count()
        ));
    }
    let luma = stats::luma(&resized);
    let grad = stats::mean_gradient_magnitude(&luma, Some(&mask)).ok_or("empty background")?;
    let (_, var) = stats::mean_variance(&luma, Some(&mask)).ok_or("empty background")?;
    Ok((-grad / 8.0).exp() * (-var.sqrt() / 64.0).exp())
}

// ------------------------------------------------------------------------------------ eyes

pub fn eyes_visible(ctx: &FaceContext<'_>, work: &Work) -> Measure {
    if work.classes.is_none() {
        return Err("no segmentation".to_owned());
    }
    let mut worst = f64::MAX;
    for (name, outer, inner) in [("right", 33usize, 133usize), ("left", 263, 362)] {
        let ellipse = work.eye_ellipse(ctx, outer, inner);
        let (mut visible, mut known) = (0usize, 0usize);
        for (i, on) in ellipse.as_slice().iter().enumerate() {
            if !on {
                continue;
            }
            if let Some(c) = work.class(i) {
                known += 1;
                if c == SegmentClass::FaceSkin {
                    visible += 1;
                }
            }
        }
        if known < 5 {
            return Err(format!("the {name} eye region is outside the image"));
        }
        worst = worst.min(visible as f64 / known as f64);
    }
    Ok(worst)
}

fn eye_aspect(ctx: &FaceContext<'_>, ids: [usize; 4]) -> f64 {
    let [outer, inner, upper, lower] = ids.map(|i| ctx.lm(i));
    let width = outer.distance(inner).max(1e-3);
    f64::from(upper.distance(lower) / width)
}

pub fn eyes_open(ctx: &FaceContext<'_>) -> Measure {
    if let Some(b) = ctx.blendshapes {
        let blink = b
            .get(Blendshape::EyeBlinkLeft)
            .max(b.get(Blendshape::EyeBlinkRight));
        return Ok(f64::from(1.0 - blink).clamp(0.0, 1.0));
    }
    let ear = eye_aspect(ctx, mesh::RIGHT_EYE_CORNERS_LIDS)
        .min(eye_aspect(ctx, mesh::LEFT_EYE_CORNERS_LIDS));
    Ok((ear / 0.28).clamp(0.0, 1.0))
}

pub fn gaze(ctx: &FaceContext<'_>) -> Measure {
    let pose = ctx.pose.ok_or("no head pose")?;
    let f = &ctx.frame;
    let mut yaw = 0.0f64;
    let mut pitch = 0.0f64;
    for (ids, iris) in [
        (mesh::RIGHT_EYE_CORNERS_LIDS, mesh::RIGHT_IRIS_CENTER),
        (mesh::LEFT_EYE_CORNERS_LIDS, mesh::LEFT_IRIS_CENTER),
    ] {
        let [outer, inner, upper, lower] = ids.map(|i| f.local(ctx.lm(i)));
        let iris = f.local(ctx.lm(iris));
        let half_w = 0.5 * (inner.x - outer.x).abs();
        if half_w < 1e-3 {
            return Err("degenerate eye".to_owned());
        }
        let h = (iris.x - 0.5 * (outer.x + inner.x)) / half_w;
        let half_open = (0.5 * (lower.y - upper.y).abs()).max(0.15 * half_w);
        let vv = (iris.y - 0.5 * (upper.y + lower.y)) / half_open;
        // Eyeball radius 12 mm; eye half-width 15 mm; half-opening 5 mm.
        yaw += f64::from((1.25 * h).clamp(-1.0, 1.0)).asin();
        pitch -= f64::from((0.42 * vv).clamp(-1.0, 1.0)).asin();
    }
    let (yaw, pitch) = (
        f64::from(pose.yaw).to_radians() + yaw / 2.0,
        f64::from(pose.pitch).to_radians() + pitch / 2.0,
    );
    Ok((yaw.cos() * pitch.cos())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees())
}

// ---------------------------------------------------------------------- mouth / expression

pub fn mouth_open(ctx: &FaceContext<'_>) -> Measure {
    let f = &ctx.frame;
    let gap = mesh::INNER_LIP_PAIRS
        .iter()
        .map(|&(up, low)| f.local(ctx.lm(low)).y - f.local(ctx.lm(up)).y)
        .fold(0.0f32, f32::max);
    let width = ctx.lm(mesh::MOUTH_RIGHT).distance(ctx.lm(mesh::MOUTH_LEFT));
    if width < 1e-3 {
        return Err("degenerate mouth".to_owned());
    }
    let lips = f64::from((2.0 * gap / width).clamp(0.0, 1.0));
    let jaw = ctx
        .blendshapes
        .map_or(0.0, |b| f64::from(b.get(Blendshape::JawOpen)));
    Ok(lips.max(jaw).clamp(0.0, 1.0))
}

/// Blendshapes that count as a non-neutral expression.
pub const EXPRESSION_BLENDSHAPES: [Blendshape; 11] = [
    Blendshape::MouthSmileLeft,
    Blendshape::MouthSmileRight,
    Blendshape::MouthFrownLeft,
    Blendshape::MouthFrownRight,
    Blendshape::BrowInnerUp,
    Blendshape::BrowOuterUpLeft,
    Blendshape::BrowOuterUpRight,
    Blendshape::NoseSneerLeft,
    Blendshape::NoseSneerRight,
    Blendshape::MouthPucker,
    Blendshape::CheekPuff,
];

pub fn expression(ctx: &FaceContext<'_>) -> Measure {
    let b = ctx.blendshapes.ok_or("no blendshapes")?;
    Ok(EXPRESSION_BLENDSHAPES
        .iter()
        .map(|s| f64::from(b.get(*s)))
        .fold(0.0, f64::max)
        .clamp(0.0, 1.0))
}

// ------------------------------------------------------------------------------- occlusion

/// The occlusion region polygon (work coordinates).
fn occlusion_polygon(ctx: &FaceContext<'_>, work: &Work) -> Vec<Point> {
    let f = &ctx.frame;
    let brow_top = mesh::RIGHT_EYEBROW
        .iter()
        .chain(&mesh::LEFT_EYEBROW)
        .map(|&i| f.local(ctx.lm(i)).y)
        .fold(f32::MAX, f32::min);
    let hairline = f.local(ctx.lm(mesh::FOREHEAD_TOP)).y;
    let clip = 0.5 * (brow_top + hairline);
    let clipped: Vec<Point> = mesh::FACE_OVAL
        .iter()
        .map(|&i| {
            let l = f.local(ctx.lm(i));
            work.to_work(f.image(Point::new(l.x, l.y.max(clip))))
        })
        .collect();
    shrink(&clipped, 0.9)
}

pub fn face_occlusion(ctx: &FaceContext<'_>, work: &Work) -> Measure {
    if work.classes.is_none() {
        return Err("no segmentation".to_owned());
    }
    let region = work.poly_mask(&occlusion_polygon(ctx, work));
    let brows = work
        .poly_mask(&work.polygon(ctx, &mesh::RIGHT_EYEBROW))
        .or(&work.poly_mask(&work.polygon(ctx, &mesh::LEFT_EYEBROW)))
        .dilate(1);
    let nose_v = ctx.frame.local(ctx.lm(mesh::NOSE_TIP)).y;
    let w = work.width() as usize;
    let (mut valid, mut occluded) = (0usize, 0usize);
    for (i, on) in region.as_slice().iter().enumerate() {
        if !on {
            continue;
        }
        let Some(c) = work.class(i) else { continue };
        if c == SegmentClass::Background {
            continue;
        }
        valid += 1;
        let occluding = match c {
            SegmentClass::BodySkin | SegmentClass::Clothes | SegmentClass::Others => true,
            SegmentClass::Hair => {
                let (x, y) = ((i % w) as u32, (i / w) as u32);
                let v = ctx
                    .frame
                    .local(work.to_image(Point::new(x as f32, y as f32)))
                    .y;
                v < nose_v && !brows.get(x, y)
            }
            _ => false,
        };
        if occluding {
            occluded += 1;
        }
    }
    if valid < MIN_PIXELS {
        return Err("face region not visible".to_owned());
    }
    Ok(occluded as f64 / valid as f64)
}

pub fn head_covering(ctx: &FaceContext<'_>, work: &Work) -> Measure {
    if work.classes.is_none() {
        return Err("no segmentation".to_owned());
    }
    let f = &ctx.frame;
    let head = &ctx.head;
    let uc = f.local(head.head.center).x;
    let half = 0.4 * head.width;
    let v_top = f.local(head.crown).y;
    let v_bottom = f.local(ctx.lm(mesh::FOREHEAD_TOP)).y;
    if v_bottom <= v_top {
        return Err("no head-top region".to_owned());
    }
    let poly: Vec<Point> = [
        (uc - half, v_top),
        (uc + half, v_top),
        (uc + half, v_bottom),
        (uc - half, v_bottom),
    ]
    .iter()
    .map(|&(u, v)| work.to_work(f.image(Point::new(u, v))))
    .collect();
    let region = work.poly_mask(&poly);
    let (mut valid, mut covered) = (0usize, 0usize);
    for (i, on) in region.as_slice().iter().enumerate() {
        if !on {
            continue;
        }
        match work.class(i) {
            None | Some(SegmentClass::Background) => {}
            Some(c) => {
                valid += 1;
                if matches!(c, SegmentClass::Others | SegmentClass::Clothes) {
                    covered += 1;
                }
            }
        }
    }
    if valid < 30 {
        return Err("head top not visible".to_owned());
    }
    Ok(covered as f64 / valid as f64)
}

pub fn eyeglasses(ctx: &FaceContext<'_>, work: &Work) -> Measure {
    if work.classes.is_none() {
        return Err("no segmentation".to_owned());
    }
    let f = &ctx.frame;
    let iod = f.iod;
    let l = |i: usize| f.local(ctx.lm(i));
    let brow_bottom = mesh::RIGHT_EYEBROW
        .iter()
        .chain(&mesh::LEFT_EYEBROW)
        .map(|&i| l(i).y)
        .fold(f32::MIN, f32::max)
        .min(-0.1 * iod);
    let (u0, u1) = (l(33).x - 0.15 * iod, l(263).x + 0.15 * iod);
    let (v0, v1) = (brow_bottom - 0.1 * iod, l(145).y.max(l(374).y) + 0.35 * iod);
    let band: Vec<Point> = [(u0, v0), (u1, v0), (u1, v1), (u0, v1)]
        .iter()
        .map(|&(u, v)| work.to_work(f.image(Point::new(u, v))))
        .collect();
    let region = work.poly_mask(&band);
    let (mut valid, mut others) = (0usize, 0usize);
    for (i, on) in region.as_slice().iter().enumerate() {
        if !on {
            continue;
        }
        match work.class(i) {
            None | Some(SegmentClass::Background) => {}
            Some(c) => {
                valid += 1;
                if c == SegmentClass::Others {
                    others += 1;
                }
            }
        }
    }
    if valid < MIN_PIXELS {
        return Err("eye region not visible".to_owned());
    }
    let seg_score = (others as f64 / valid as f64 / 0.08).clamp(0.0, 1.0);

    // Bridge edge: horizontal edges (vertical gradient) at the nose bridge relative to the
    // forehead's texture.
    let luma = stats::luma(&work.image);
    let (_, gy) = stats::sobel(&luma);
    let grad = stats::gradient_magnitude(&luma);
    let patch_mean =
        |plane: &Plane, center: Point, half: f32, abs: bool, scale: f32| -> Option<f64> {
            let c = work.to_work(center);
            let h = half * work.scale;
            let (mut s, mut n) = (0.0f64, 0usize);
            for y in (c.y - h).round().max(0.0) as u32
                ..((c.y + h).round().max(0.0) as u32).min(work.height())
            {
                for x in (c.x - h).round().max(0.0) as u32
                    ..((c.x + h).round().max(0.0) as u32).min(work.width())
                {
                    let v = plane.get(x, y);
                    s += f64::from(if abs { v.abs() } else { v }) * f64::from(scale);
                    n += 1;
                }
            }
            (n > 0).then(|| s / n as f64)
        };
    let bridge = patch_mean(
        &gy,
        ctx.lm(mesh::MIDWAY_BETWEEN_EYES),
        0.15 * iod,
        true,
        0.125,
    );
    let forehead = patch_mean(&grad, ctx.lm(151), 0.15 * iod, false, 1.0);
    let edge_score = match (bridge, forehead) {
        (Some(b), Some(fh)) => ((b / (fh + 1.0) - 1.5) / 2.0).clamp(0.0, 1.0),
        _ => 0.0,
    };
    Ok(seg_score.max(0.5 * edge_score))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::head_geometry;
    use crate::pose::PoseEstimator;
    use crate::pose::tests::{canonical, synthetic_canonical};
    use face_imaging::transform::PixelRect;
    use image::Rgb;

    /// A synthetic frontal face: canonical landmarks projected into a 400x400 frame, drawn as a
    /// uniform skin-coloured oval on a background; segmentation from the same geometry.
    struct Synthetic {
        image: RgbImage,
        landmarks: Vec<Point3>,
        seg: Segmentation,
    }

    fn synthetic(skin: [u8; 3], background: impl Fn(u32, u32) -> [u8; 3]) -> Synthetic {
        let est = PoseEstimator::new(canonical().unwrap_or_else(synthetic_canonical));
        let landmarks = est.synthesize_landmarks(Pose::default(), (200.0, 190.0), 7.0);
        let oval: Vec<Point> = mesh::FACE_OVAL.iter().map(|&i| landmarks[i].xy()).collect();
        let face = Mask::from_polygon(400, 400, &oval);
        let image = RgbImage::from_fn(400, 400, |x, y| {
            if face.get(x, y) {
                Rgb(skin)
            } else {
                Rgb(background(x, y))
            }
        });
        let roi = PixelRect {
            x: 0,
            y: 0,
            width: 400,
            height: 400,
        };
        let classes = (0..400 * 400)
            .map(|i| if face.get(i % 400, i / 400) { 3 } else { 0 })
            .collect();
        Synthetic {
            image,
            landmarks,
            seg: Segmentation::from_classes(roi, classes).unwrap(),
        }
    }

    fn context<'a>(s: &'a Synthetic, with_seg: bool) -> FaceContext<'a> {
        let seg = with_seg.then_some(&s.seg);
        FaceContext {
            image: &s.image,
            landmarks: &s.landmarks,
            frame: FaceFrame::from_landmarks(&s.landmarks).unwrap(),
            head: head_geometry(&s.landmarks, seg).unwrap(),
            blendshapes: None,
            pose: Some(Pose::default()),
            segmentation: seg,
        }
    }

    #[test]
    fn illumination_of_a_uniform_face() {
        if canonical().is_none() {
            return; // needs a face-shaped mesh
        }
        let s = synthetic([128, 128, 128], |_, _| [30, 60, 200]);
        let ctx = context(&s, true);
        let work = Work::new(&ctx).unwrap();
        let ill = illumination(&ctx, &work);
        // sRGB 128 -> linear Y 0.2159 -> 55 -> 55/255.
        assert!((ill.luminance_mean.unwrap() - 55.0 / 255.0).abs() < 1e-3);
        assert_eq!(ill.under_exposure.unwrap(), 0.0);
        assert_eq!(ill.over_exposure.unwrap(), 0.0);
        assert!(ill.dynamic_range.unwrap() < 0.05);
        assert!((ill.uniformity.unwrap() - 1.0).abs() < 1e-6);
        // A dark face is under-exposed; a white one over-exposed.
        let dark = synthetic([20, 20, 20], |_, _| [128, 128, 128]);
        let ctx = context(&dark, true);
        let ill = illumination(&ctx, &Work::new(&ctx).unwrap());
        assert!(ill.under_exposure.unwrap() > 0.99);
        let white = synthetic([255, 255, 255], |_, _| [128, 128, 128]);
        let ctx = context(&white, true);
        let ill = illumination(&ctx, &Work::new(&ctx).unwrap());
        assert!(ill.over_exposure.unwrap() > 0.9);
    }

    #[test]
    fn half_lit_face_is_not_uniform() {
        if canonical().is_none() {
            return;
        }
        let mut s = synthetic([180, 150, 130], |_, _| [128, 128, 128]);
        // Darken the image-left half (subject's right cheek).
        for (x, _, p) in s.image.enumerate_pixels_mut() {
            if x < 200 && p.0 == [180, 150, 130] {
                *p = Rgb([60, 50, 43]);
            }
        }
        let ctx = context(&s, true);
        let u = illumination(&ctx, &Work::new(&ctx).unwrap())
            .uniformity
            .unwrap();
        // Linear luma: dark side ~9, lit side ~85.
        assert!(u < 0.2, "{u}");
    }

    #[test]
    fn background_occlusion_and_eyes_on_synthetic_segmentation() {
        if canonical().is_none() {
            return;
        }
        let plain = synthetic([180, 150, 130], |_, _| [200, 200, 200]);
        let busy = synthetic([180, 150, 130], |x, y| {
            if (x / 3 + y / 3) % 2 == 0 {
                [20, 20, 20]
            } else {
                [230, 230, 230]
            }
        });
        let bp = background_uniformity(&context(&plain, true)).unwrap();
        let bb = background_uniformity(&context(&busy, true)).unwrap();
        assert!(bp > 0.95 && bb < 0.1, "{bp} {bb}");

        let ctx = context(&plain, true);
        let work = Work::new(&ctx).unwrap();
        assert!(face_occlusion(&ctx, &work).unwrap() < 0.01);
        assert!(eyes_visible(&ctx, &work).unwrap() > 0.99);
        assert!(face_occlusion(&context(&plain, false), &work).is_ok());
        // Without segmentation the segmentation-based measures are undetermined.
        let no_seg = context(&plain, false);
        let w2 = Work::new(&no_seg).unwrap();
        assert!(eyes_visible(&no_seg, &w2).is_err());
        assert!(face_occlusion(&no_seg, &w2).is_err());
        assert!(background_uniformity(&no_seg).is_err());

        // Paint "others" (e.g. a mask) over the lower face: occlusion rises.
        let mut masked = synthetic([180, 150, 130], |_, _| [200, 200, 200]);
        let nose_y = masked.landmarks[mesh::NOSE_TIP].y as u32;
        let classes: Vec<u8> = masked
            .seg
            .classes()
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if *c == 3 && (i / 400) as u32 > nose_y {
                    5
                } else {
                    *c
                }
            })
            .collect();
        masked.seg = Segmentation::from_classes(masked.seg.roi, classes).unwrap();
        let ctx = context(&masked, true);
        let occ = face_occlusion(&ctx, &Work::new(&ctx).unwrap()).unwrap();
        assert!(occ > 0.25, "{occ}");
    }

    #[test]
    fn sharpness_drops_with_blur() {
        if canonical().is_none() {
            return;
        }
        // Textured face: noise inside the oval.
        let s = synthetic([128, 128, 128], |_, _| [128, 128, 128]);
        let oval: Vec<Point> = mesh::FACE_OVAL
            .iter()
            .map(|&i| s.landmarks[i].xy())
            .collect();
        let face = Mask::from_polygon(400, 400, &oval);
        let textured = RgbImage::from_fn(400, 400, |x, y| {
            let n = ((x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) >> 7) % 120;
            let v = if face.get(x, y) { 70 + n as u8 } else { 128 };
            Rgb([v, v, v])
        });
        let blurred = transform::resize(
            &transform::resize(&textured, 100, 100, Filter::Area),
            400,
            400,
            Filter::Bilinear,
        );
        let sharp_s = Synthetic {
            image: textured,
            landmarks: s.landmarks.clone(),
            seg: s.seg.clone(),
        };
        let blur_s = Synthetic {
            image: blurred,
            landmarks: s.landmarks.clone(),
            seg: s.seg.clone(),
        };
        let c1 = context(&sharp_s, true);
        let c2 = context(&blur_s, true);
        let a = sharpness(&c1, &Work::new(&c1).unwrap()).unwrap();
        let b = sharpness(&c2, &Work::new(&c2).unwrap()).unwrap();
        assert!(a > b + 0.2, "{a} {b}");
    }

    #[test]
    fn landmark_measures() {
        if canonical().is_none() {
            return;
        }
        let s = synthetic([128, 128, 128], |_, _| [128, 128, 128]);
        let mut ctx = context(&s, false);
        // The canonical mesh has slightly parted lips (gap 0.55 cm over a 4.9 cm mouth): 0.22.
        // Real closed mouths read about 0.01-0.02 (golden landmarks), toothy smiles 0.2-0.75.
        let mouth = mouth_open(&ctx).unwrap();
        assert!((mouth - 0.22).abs() < 0.03, "{mouth}");
        let g = gaze(&ctx).unwrap();
        assert!(g < 10.0, "{g}");
        assert!(expression(&ctx).is_err());
        let mut scores = [0.0f32; 52];
        scores[Blendshape::EyeBlinkLeft.index()] = 0.7;
        scores[Blendshape::MouthSmileRight.index()] = 0.8;
        scores[Blendshape::BrowDownLeft.index()] = 0.95;
        scores[Blendshape::JawOpen.index()] = 0.6;
        let b = BlendshapeScores(scores);
        ctx.blendshapes = Some(&b);
        assert!((eyes_open(&ctx).unwrap() - 0.3).abs() < 1e-6);
        assert!((expression(&ctx).unwrap() - 0.8).abs() < 1e-6);
        assert!((mouth_open(&ctx).unwrap() - 0.6).abs() < 1e-6);
        let r = geometry_ratios(&ctx.head, 400, 400);
        assert!((r.eye_mid_horizontal - 0.5).abs() < 0.02);
        assert!(r.head_in_frame > 0.0);
    }
}
