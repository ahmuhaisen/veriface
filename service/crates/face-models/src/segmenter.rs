//! MediaPipe selfie multiclass segmentation (256x256): background, hair, body skin, face skin,
//! clothes, others (accessories).
//!
//! The pipeline runs it on an axis-aligned pixel ROI ([`segmentation_roi`]). The ROI is stretched
//! to 256x256 with MediaPipe's warp (bilinear, replicated border), normalised per the manifest, and
//! the category mask is computed at ROI resolution exactly like MediaPipe Tasks'
//! `ProcessForCategoryMaskCpu`: the raw logits are bilinearly interpolated with aligned corners
//! (`src = dst * (in - 1) / (out - 1)`), then the argmax is taken.

use std::sync::Arc;

use face_imaging::RgbImage;
use face_imaging::geom::Point;
use face_imaging::transform::{Border, PixelRect, crop_rotated};

use crate::error::ModelError;
use crate::runtime::ModelSession;
use crate::tensor::{ImageInputSpec, Normalization};

/// Number of classes.
pub const NUM_CLASSES: usize = 6;

/// Segmentation classes, in model channel order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum SegmentClass {
    Background = 0,
    Hair = 1,
    BodySkin = 2,
    FaceSkin = 3,
    Clothes = 4,
    /// Accessories: glasses, hats, masks, jewellery, objects.
    Others = 5,
}

impl SegmentClass {
    pub const ALL: [SegmentClass; NUM_CLASSES] = [
        SegmentClass::Background,
        SegmentClass::Hair,
        SegmentClass::BodySkin,
        SegmentClass::FaceSkin,
        SegmentClass::Clothes,
        SegmentClass::Others,
    ];

    /// The class with this id (ids above 5 map to `Others`).
    pub fn from_id(id: u8) -> SegmentClass {
        match id {
            0 => SegmentClass::Background,
            1 => SegmentClass::Hair,
            2 => SegmentClass::BodySkin,
            3 => SegmentClass::FaceSkin,
            4 => SegmentClass::Clothes,
            _ => SegmentClass::Others,
        }
    }

    /// The model's label, e.g. `face-skin`.
    pub fn label(self) -> &'static str {
        match self {
            SegmentClass::Background => "background",
            SegmentClass::Hair => "hair",
            SegmentClass::BodySkin => "body-skin",
            SegmentClass::FaceSkin => "face-skin",
            SegmentClass::Clothes => "clothes",
            SegmentClass::Others => "others",
        }
    }
}

/// The pipeline's segmentation ROI (binding rule, `testdata/golden/README.md` "Segmentation ROI"):
/// with `E` the midpoint of the iris centres and `IOD` their distance, a square of side `6 IOD`
/// centred at `(E.x, E.y + 0.5 IOD)`, clamped (shrunk, never padded) to the image with floor/ceil.
pub fn segmentation_roi(
    right_iris: Point,
    left_iris: Point,
    width: u32,
    height: u32,
) -> Option<PixelRect> {
    let iod = right_iris.distance(left_iris);
    if !(iod.is_finite() && iod > 0.0) {
        return None;
    }
    let e = right_iris.midpoint(left_iris);
    PixelRect::square_clamped(Point::new(e.x, e.y + 0.5 * iod), 6.0 * iod, width, height)
}

/// A category mask over an image region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segmentation {
    /// The image region the mask covers; mask pixel `(i, j)` is image pixel `(x + i, y + j)`.
    pub roi: PixelRect,
    classes: Vec<u8>,
}

impl Segmentation {
    /// Wrap a class-id mask of the ROI's size (e.g. a golden mask). `None` on a size mismatch.
    pub fn from_classes(roi: PixelRect, classes: Vec<u8>) -> Option<Segmentation> {
        (classes.len() == roi.width as usize * roi.height as usize)
            .then_some(Segmentation { roi, classes })
    }

    /// Class ids, row-major, `roi.width` x `roi.height`.
    pub fn classes(&self) -> &[u8] {
        &self.classes
    }

    /// Class at mask pixel `(i, j)`.
    pub fn class(&self, i: u32, j: u32) -> SegmentClass {
        SegmentClass::from_id(self.classes[j as usize * self.roi.width as usize + i as usize])
    }

    /// Class at an image point (nearest pixel); `None` outside the ROI.
    pub fn class_at(&self, p: Point) -> Option<SegmentClass> {
        let (x, y) = (p.x.round(), p.y.round());
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        let (i, j) = (x - self.roi.x as f32, y - self.roi.y as f32);
        if i < 0.0 || j < 0.0 || i >= self.roi.width as f32 || j >= self.roi.height as f32 {
            return None;
        }
        Some(self.class(i as u32, j as u32))
    }

    /// Share of mask pixels per class, indexed by class id.
    pub fn fractions(&self) -> [f32; NUM_CLASSES] {
        let mut counts = [0usize; NUM_CLASSES];
        for c in &self.classes {
            counts[usize::from(*c).min(NUM_CLASSES - 1)] += 1;
        }
        let n = self.classes.len().max(1) as f32;
        counts.map(|c| c as f32 / n)
    }
}

/// Category mask from NHWC logits (`in_h` x `in_w` x [`NUM_CLASSES`]) at `out_w` x `out_h`, as
/// MediaPipe's `ProcessForCategoryMaskCpu`: aligned-corner bilinear interpolation of the raw
/// logits, then argmax (first maximum wins).
pub fn category_mask(
    logits: &[f32],
    in_w: usize,
    in_h: usize,
    out_w: usize,
    out_h: usize,
) -> Vec<u8> {
    let scale = |inp: usize, out: usize| {
        if out > 1 {
            (inp as f32 - 1.0) / (out as f32 - 1.0)
        } else {
            0.0
        }
    };
    let (sx, sy) = (scale(in_w, out_w), scale(in_h, out_h));
    let at = |x: usize, y: usize, c: usize| logits[(y * in_w + x) * NUM_CLASSES + c];
    let mut out = Vec::with_capacity(out_w * out_h);
    for j in 0..out_h {
        let fy = j as f32 * sy;
        let y0 = fy.floor().max(0.0) as usize;
        let y1 = (fy.ceil().min(in_h as f32 - 1.0)) as usize;
        let t0 = (fy - y0 as f32).clamp(0.0, 1.0);
        for i in 0..out_w {
            let fx = i as f32 * sx;
            let x0 = fx.floor().max(0.0) as usize;
            let x1 = (fx.ceil().min(in_w as f32 - 1.0)) as usize;
            let t1 = (fx - x0 as f32).clamp(0.0, 1.0);
            let mut best = (0usize, f32::NEG_INFINITY);
            for c in 0..NUM_CLASSES {
                let v = at(x0, y0, c) * (1.0 - t0) * (1.0 - t1)
                    + at(x0, y1, c) * t0 * (1.0 - t1)
                    + at(x1, y0, c) * (1.0 - t0) * t1
                    + at(x1, y1, c) * t0 * t1;
                if v > best.1 {
                    best = (c, v);
                }
            }
            out.push(best.0 as u8);
        }
    }
    out
}

/// The segmentation model bound to a session.
#[derive(Debug, Clone)]
pub struct Segmenter {
    session: Arc<ModelSession>,
    spec: ImageInputSpec,
    output: usize,
    /// Output is NCHW (`[1, 6, H, W]`) instead of NHWC (`[1, H, W, 6]`).
    channels_first: bool,
}

impl Segmenter {
    pub fn new(session: Arc<ModelSession>) -> Result<Segmenter, ModelError> {
        let id = session.id().to_owned();
        let bad = |reason: String| ModelError::Spec {
            id: id.clone(),
            reason,
        };
        let input = session
            .entry()
            .io
            .inputs
            .first()
            .ok_or_else(|| bad("no input".to_owned()))?;
        let spec = ImageInputSpec::from_tensor_spec(&id, input, Some(Normalization::SYMMETRIC))?;
        let output = session.output_index("logits").unwrap_or(0);
        let shape = &session
            .outputs()
            .get(output)
            .ok_or_else(|| bad("no output".to_owned()))?
            .shape;
        let channels_first = match shape.as_slice() {
            [_, h, w, c]
                if *c == NUM_CLASSES as i64
                    && *h == i64::from(spec.height)
                    && *w == i64::from(spec.width) =>
            {
                false
            }
            [_, c, h, w]
                if *c == NUM_CLASSES as i64
                    && *h == i64::from(spec.height)
                    && *w == i64::from(spec.width) =>
            {
                true
            }
            other => return Err(bad(format!("unexpected logits shape {other:?}"))),
        };
        Ok(Segmenter {
            session,
            spec,
            output,
            channels_first,
        })
    }

    pub fn session(&self) -> &Arc<ModelSession> {
        &self.session
    }

    /// The uint8 tensor image the model sees for `roi` (before normalisation).
    pub fn preprocess(&self, image: &RgbImage, roi: &PixelRect) -> RgbImage {
        crop_rotated(
            image,
            &roi.to_rotated(),
            self.spec.width,
            self.spec.height,
            Border::Replicate,
        )
    }

    /// Raw logits (NHWC, `256 x 256 x 6`) for `roi`.
    pub fn logits(&self, image: &RgbImage, roi: &PixelRect) -> Result<Vec<f32>, ModelError> {
        if roi.width == 0 || roi.height == 0 {
            return Err(ModelError::InvalidInput(
                "empty segmentation ROI".to_owned(),
            ));
        }
        let input = self.spec.to_tensor(&self.preprocess(image, roi))?;
        let mut outputs = self.session.run(&[input])?;
        let raw = std::mem::replace(&mut outputs[self.output], crate::TensorF32::zeros(vec![0]))
            .into_data();
        if !self.channels_first {
            return Ok(raw);
        }
        let plane = (self.spec.width * self.spec.height) as usize;
        let mut nhwc = vec![0.0f32; raw.len()];
        for c in 0..NUM_CLASSES {
            for p in 0..plane {
                nhwc[p * NUM_CLASSES + c] = raw[c * plane + p];
            }
        }
        Ok(nhwc)
    }

    /// Category mask of `roi` at ROI resolution.
    pub fn segment(&self, image: &RgbImage, roi: &PixelRect) -> Result<Segmentation, ModelError> {
        let logits = self.logits(image, roi)?;
        let classes = category_mask(
            &logits,
            self.spec.width as usize,
            self.spec.height as usize,
            roi.width as usize,
            roi.height as usize,
        );
        Ok(Segmentation { roi: *roi, classes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roi_follows_the_binding_rule() {
        // IOD 20, E = (100, 100): centre (100, 110), side 120 -> [40, 50, 120, 120].
        let r =
            segmentation_roi(Point::new(90.0, 100.0), Point::new(110.0, 100.0), 400, 400).unwrap();
        assert_eq!(
            r,
            PixelRect {
                x: 40,
                y: 50,
                width: 120,
                height: 120
            }
        );
        // Clamped at the image edge: shrinks, never pads.
        let r = segmentation_roi(Point::new(0.0, 10.0), Point::new(20.0, 10.0), 400, 400).unwrap();
        assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 70, 80));
        assert!(segmentation_roi(Point::new(5.0, 5.0), Point::new(5.0, 5.0), 10, 10).is_none());
    }

    #[test]
    fn category_mask_interpolates_logits_with_aligned_corners() {
        // 2x1 input: left pixel favours class 1, right pixel class 4.
        let mut logits = vec![0.0f32; 2 * NUM_CLASSES];
        logits[1] = 5.0;
        logits[NUM_CLASSES + 4] = 4.0;
        let mask = category_mask(&logits, 2, 1, 5, 1);
        // Aligned corners: output 0 = input 0, output 4 = input 1, output 2 halfway (2.5 vs 2.0).
        assert_eq!(mask, vec![1, 1, 1, 4, 4]);
        // Identity size keeps every pixel.
        assert_eq!(category_mask(&logits, 2, 1, 2, 1), vec![1, 4]);
        let seg = Segmentation::from_classes(
            PixelRect {
                x: 10,
                y: 20,
                width: 5,
                height: 1,
            },
            mask,
        )
        .unwrap();
        assert_eq!(
            seg.class_at(Point::new(10.0, 20.0)),
            Some(SegmentClass::Hair)
        );
        assert_eq!(
            seg.class_at(Point::new(14.2, 20.4)),
            Some(SegmentClass::Clothes)
        );
        assert_eq!(seg.class_at(Point::new(9.0, 20.0)), None);
        let f = seg.fractions();
        assert!((f[1] - 0.6).abs() < 1e-6 && (f[4] - 0.4).abs() < 1e-6);
        assert_eq!(SegmentClass::from_id(3).label(), "face-skin");
    }
}
