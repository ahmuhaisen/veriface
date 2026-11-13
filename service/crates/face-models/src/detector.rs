//! BlazeFace face detection (MediaPipe short-range 128x128 and full-range 192x192).
//!
//! Pre- and post-processing follow MediaPipe's `FaceDetectorGraph` (Tasks) exactly:
//! - the ROI (whole image by default) is padded to the square tensor aspect, warped corner to
//!   corner with bilinear sampling and a black border, then normalised per the manifest;
//! - SSD anchors from `SsdAnchorsCalculator` (fixed size, offset 0.5);
//! - box and six-keypoint decoding from `TensorsToDetectionsCalculator` (x before y, divided by
//!   the input size), score = sigmoid(clamp(logit, ±100));
//! - weighted non-maximum suppression (`NonMaxSuppressionCalculator`, IoU, WEIGHTED);
//! - projection back through the ROI.

use std::sync::Arc;

use face_imaging::RgbImage;
use face_imaging::geom::{Affine, Point, Rect};
use face_imaging::transform::{Border, RotatedRect, crop_rotated};

use crate::error::ModelError;
use crate::runtime::ModelSession;
use crate::tensor::{ImageInputSpec, Normalization};

/// Keypoint indices of a [`Detection`]. "Right" and "left" are the subject's.
pub mod keypoint {
    /// Subject's right eye (image-left in a non-mirrored photo).
    pub const RIGHT_EYE: usize = 0;
    /// Subject's left eye (image-right).
    pub const LEFT_EYE: usize = 1;
    pub const NOSE_TIP: usize = 2;
    pub const MOUTH_CENTER: usize = 3;
    pub const RIGHT_EAR_TRAGION: usize = 4;
    pub const LEFT_EAR_TRAGION: usize = 5;
}

/// One detected face, in the coordinates of the image passed to the detector.
#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    /// Confidence, 0-1.
    pub score: f32,
    /// Face box (MediaPipe's box: brows to chin, roughly ear to ear).
    pub bbox: Rect,
    /// Six keypoints, indexed by [`keypoint`].
    pub keypoints: [Point; 6],
}

impl Detection {
    /// Subject's right eye (image-left).
    pub fn right_eye(&self) -> Point {
        self.keypoints[keypoint::RIGHT_EYE]
    }

    /// Subject's left eye (image-right).
    pub fn left_eye(&self) -> Point {
        self.keypoints[keypoint::LEFT_EYE]
    }

    /// Distance between the two eye keypoints, in pixels.
    pub fn inter_eye_distance(&self) -> f32 {
        self.right_eye().distance(self.left_eye())
    }

    /// The landmark-model ROI MediaPipe derives from this detection (`FaceDetectorGraph`
    /// expanded rect): box centre, box size x1.5 (not squared), rotated so the line from the
    /// right-eye keypoint to the left-eye keypoint is horizontal.
    pub fn landmark_roi(&self) -> RotatedRect {
        landmark_roi(&self.bbox, self.right_eye(), self.left_eye())
    }
}

/// MediaPipe's landmark ROI from a face box and the two eye keypoints (subject's right eye, i.e.
/// image-left, first).
pub fn landmark_roi(bbox: &Rect, right_eye: Point, left_eye: Point) -> RotatedRect {
    let rotation =
        normalize_radians(-(-(left_eye.y - right_eye.y)).atan2(left_eye.x - right_eye.x));
    RotatedRect {
        center: bbox.center(),
        width: bbox.width * 1.5,
        height: bbox.height * 1.5,
        rotation,
    }
}

/// Into `[-pi, pi)`, as MediaPipe's `NormalizeRadians`.
pub fn normalize_radians(angle: f32) -> f32 {
    use std::f32::consts::PI;
    angle - 2.0 * PI * ((angle + PI) / (2.0 * PI)).floor()
}

/// Detector thresholds.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectorConfig {
    /// Minimum score to keep a detection (MediaPipe Tasks `min_detection_confidence`, default 0.5).
    pub min_score: f32,
    /// IoU above which detections are merged by weighted NMS (MediaPipe default 0.3).
    pub min_suppression_iou: f32,
    /// Keep at most this many detections (highest scores first). `None`: all.
    pub max_results: Option<usize>,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        DetectorConfig {
            min_score: 0.5,
            min_suppression_iou: 0.3,
            max_results: None,
        }
    }
}

/// `SsdAnchorsCalculator` options (fixed-size anchors).
#[derive(Clone, Debug, PartialEq)]
pub struct AnchorOptions {
    pub input_size: u32,
    pub strides: Vec<u32>,
    pub aspect_ratios: Vec<f32>,
    pub interpolated_scale_aspect_ratio: f32,
    pub anchor_offset: f32,
}

impl AnchorOptions {
    /// Short-range BlazeFace: 128x128, strides 8/16/16/16, 896 anchors.
    pub fn short_range() -> AnchorOptions {
        AnchorOptions {
            input_size: 128,
            strides: vec![8, 16, 16, 16],
            aspect_ratios: vec![1.0],
            interpolated_scale_aspect_ratio: 1.0,
            anchor_offset: 0.5,
        }
    }

    /// Full-range BlazeFace: 192x192, stride 4, 2304 anchors.
    pub fn full_range() -> AnchorOptions {
        AnchorOptions {
            input_size: 192,
            strides: vec![4],
            aspect_ratios: vec![1.0],
            interpolated_scale_aspect_ratio: 0.0,
            anchor_offset: 0.5,
        }
    }

    /// The MediaPipe configuration for a model input size (128 or 192).
    pub fn for_input_size(size: u32) -> Option<AnchorOptions> {
        match size {
            128 => Some(AnchorOptions::short_range()),
            192 => Some(AnchorOptions::full_range()),
            _ => None,
        }
    }

    /// Anchor centres in normalised tensor coordinates, in MediaPipe's order (layer group, row,
    /// column, anchor within the cell). Fixed-size anchors have width = height = 1.
    pub fn anchors(&self) -> Vec<Point> {
        let mut out = Vec::new();
        let mut layer = 0;
        while layer < self.strides.len() {
            // Consecutive layers with the same stride share one feature map.
            let mut per_cell = 0;
            let mut last = layer;
            while last < self.strides.len() && self.strides[last] == self.strides[layer] {
                per_cell += self.aspect_ratios.len();
                if self.interpolated_scale_aspect_ratio > 0.0 {
                    per_cell += 1;
                }
                last += 1;
            }
            let fm = self.input_size.div_ceil(self.strides[layer]);
            for y in 0..fm {
                for x in 0..fm {
                    let c = Point::new(
                        (x as f32 + self.anchor_offset) / fm as f32,
                        (y as f32 + self.anchor_offset) / fm as f32,
                    );
                    out.extend(std::iter::repeat_n(c, per_cell));
                }
            }
            layer = last;
        }
        out
    }
}

/// A decoded detection in normalised tensor coordinates (before projection to the image).
#[derive(Clone, Debug, PartialEq)]
pub struct TensorDetection {
    pub score: f32,
    pub xmin: f32,
    pub ymin: f32,
    pub xmax: f32,
    pub ymax: f32,
    pub keypoints: [Point; 6],
}

impl TensorDetection {
    fn area(&self) -> f32 {
        (self.xmax - self.xmin).max(0.0) * (self.ymax - self.ymin).max(0.0)
    }

    fn iou(&self, o: &TensorDetection) -> f32 {
        let iw = self.xmax.min(o.xmax) - self.xmin.max(o.xmin);
        let ih = self.ymax.min(o.ymax) - self.ymin.max(o.ymin);
        if iw <= 0.0 || ih <= 0.0 {
            return 0.0;
        }
        let inter = iw * ih;
        let union = self.area() + o.area() - inter;
        if union > 0.0 { inter / union } else { 0.0 }
    }
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Decode raw BlazeFace outputs (`regressors` = N x 16, `classificators` = N x 1) against
/// `anchors` with scale `input_size`. Keeps detections with score >= `min_score` and a
/// non-negative box size, in anchor order.
pub fn decode(
    regressors: &[f32],
    classificators: &[f32],
    anchors: &[Point],
    input_size: f32,
    min_score: f32,
) -> Vec<TensorDetection> {
    let mut out = Vec::new();
    for (i, a) in anchors.iter().enumerate() {
        let (Some(raw), Some(logit)) = (regressors.get(i * 16..i * 16 + 16), classificators.get(i))
        else {
            break;
        };
        let score = sigmoid(logit.clamp(-100.0, 100.0));
        if score.is_nan() || score < min_score {
            continue;
        }
        let xc = raw[0] / input_size + a.x;
        let yc = raw[1] / input_size + a.y;
        let w = raw[2] / input_size;
        let h = raw[3] / input_size;
        if w.is_nan() || h.is_nan() || w < 0.0 || h < 0.0 {
            continue;
        }
        let mut keypoints = [Point::default(); 6];
        for (k, kp) in keypoints.iter_mut().enumerate() {
            *kp = Point::new(
                raw[4 + 2 * k] / input_size + a.x,
                raw[5 + 2 * k] / input_size + a.y,
            );
        }
        out.push(TensorDetection {
            score,
            xmin: xc - w * 0.5,
            ymin: yc - h * 0.5,
            xmax: xc + w * 0.5,
            ymax: yc + h * 0.5,
            keypoints,
        });
    }
    out
}

/// MediaPipe's weighted non-maximum suppression: repeatedly take the highest-scoring detection,
/// merge every remaining detection whose IoU with it exceeds `iou_threshold` (itself included)
/// into a score-weighted average box and keypoints, keep the top score. Output is in descending
/// score order.
pub fn weighted_nms(mut dets: Vec<TensorDetection>, iou_threshold: f32) -> Vec<TensorDetection> {
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut out = Vec::new();
    while !dets.is_empty() {
        let top = dets[0].clone();
        let (candidates, remaining): (Vec<_>, Vec<_>) =
            dets.into_iter().partition(|d| d.iou(&top) > iou_threshold);
        let mut merged = top.clone();
        if !candidates.is_empty() {
            let total: f32 = candidates.iter().map(|c| c.score).sum();
            let (mut xmin, mut ymin, mut xmax, mut ymax) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            let mut kps = [Point::default(); 6];
            for c in &candidates {
                xmin += c.xmin * c.score;
                ymin += c.ymin * c.score;
                xmax += c.xmax * c.score;
                ymax += c.ymax * c.score;
                for (k, kp) in kps.iter_mut().enumerate() {
                    *kp = *kp + c.keypoints[k] * c.score;
                }
            }
            merged.xmin = xmin / total;
            merged.ymin = ymin / total;
            merged.xmax = xmax / total;
            merged.ymax = ymax / total;
            merged.keypoints = kps.map(|p| p * (1.0 / total));
        }
        out.push(merged);
        if remaining.is_empty() || candidates.is_empty() {
            // `candidates` always holds `top` itself (IoU 1) unless its box is degenerate; stop
            // rather than loop forever in that case.
            if candidates.is_empty() {
                break;
            }
        }
        dets = remaining;
    }
    out
}

/// BlazeFace detector bound to a model session.
#[derive(Debug, Clone)]
pub struct FaceDetector {
    session: Arc<ModelSession>,
    spec: ImageInputSpec,
    anchors: Vec<Point>,
    config: DetectorConfig,
    regressors: usize,
    classificators: usize,
}

impl FaceDetector {
    /// Wrap a BlazeFace session. The input size (128 or 192, from the manifest) selects the anchor
    /// configuration; the anchor count must equal the model's box count.
    pub fn new(
        session: Arc<ModelSession>,
        config: DetectorConfig,
    ) -> Result<FaceDetector, ModelError> {
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
        if spec.width != spec.height {
            return Err(bad("the detector input must be square".to_owned()));
        }
        let anchors = AnchorOptions::for_input_size(spec.width)
            .ok_or_else(|| {
                bad(format!(
                    "no anchor configuration for input size {}",
                    spec.width
                ))
            })?
            .anchors();
        let find = |name: &str, last: i64| {
            session.output_index(name).or_else(|| {
                session
                    .outputs()
                    .iter()
                    .position(|o| o.shape.last() == Some(&last))
            })
        };
        let regressors =
            find("regressors", 16).ok_or_else(|| bad("no regressors output".into()))?;
        let classificators =
            find("classificators", 1).ok_or_else(|| bad("no classificators output".into()))?;
        let boxes = session.outputs()[regressors]
            .shape
            .get(1)
            .copied()
            .unwrap_or(-1);
        if boxes >= 0 && boxes as usize != anchors.len() {
            return Err(bad(format!(
                "the model has {boxes} boxes but the anchor configuration has {}",
                anchors.len()
            )));
        }
        Ok(FaceDetector {
            session,
            spec,
            anchors,
            config,
            regressors,
            classificators,
        })
    }

    /// Square input size in pixels (128 or 192).
    pub fn input_size(&self) -> u32 {
        self.spec.width
    }

    pub fn config(&self) -> &DetectorConfig {
        &self.config
    }

    pub fn session(&self) -> &Arc<ModelSession> {
        &self.session
    }

    /// The uint8 tensor image the model sees for `roi` (before normalisation) and the map from
    /// tensor pixels to image pixels: the ROI is padded to the square aspect, warped corner to
    /// corner, bilinear, black outside the image.
    pub fn preprocess(&self, image: &RgbImage, roi: &RotatedRect) -> (RgbImage, Affine) {
        let s = self.spec.width;
        let (padded, _) = roi.pad_to_aspect(s, s);
        let tensor_image = crop_rotated(image, &padded, s, s, Border::Constant([0, 0, 0]));
        (tensor_image, padded.output_to_source(s, s))
    }

    /// The whole-image ROI MediaPipe uses when none is given.
    pub fn whole_image_roi(image: &RgbImage) -> RotatedRect {
        RotatedRect {
            center: Point::new(image.width() as f32 * 0.5, image.height() as f32 * 0.5),
            width: image.width() as f32,
            height: image.height() as f32,
            rotation: 0.0,
        }
    }

    /// Detect faces in the whole image.
    pub fn detect(&self, image: &RgbImage) -> Result<Vec<Detection>, ModelError> {
        self.detect_roi(image, &FaceDetector::whole_image_roi(image))
    }

    /// Detect faces inside `roi`; results are in image coordinates.
    pub fn detect_roi(
        &self,
        image: &RgbImage,
        roi: &RotatedRect,
    ) -> Result<Vec<Detection>, ModelError> {
        if image.width() == 0 || image.height() == 0 || roi.width <= 0.0 || roi.height <= 0.0 {
            return Err(ModelError::InvalidInput("empty image or ROI".to_owned()));
        }
        let (tensor_image, to_image) = self.preprocess(image, roi);
        let input = self.spec.to_tensor(&tensor_image)?;
        let outputs = self.session.run(&[input])?;
        Ok(self.postprocess(
            outputs[self.regressors].data(),
            outputs[self.classificators].data(),
            &to_image,
        ))
    }

    /// Decode, NMS and project raw outputs; `to_image` maps tensor pixels to image pixels.
    pub fn postprocess(
        &self,
        regressors: &[f32],
        classificators: &[f32],
        to_image: &Affine,
    ) -> Vec<Detection> {
        let s = self.spec.width as f32;
        let decoded = decode(
            regressors,
            classificators,
            &self.anchors,
            s,
            self.config.min_score,
        );
        let mut kept = weighted_nms(decoded, self.config.min_suppression_iou);
        if let Some(max) = self.config.max_results {
            kept.truncate(max);
        }
        kept.into_iter()
            .map(|d| {
                let corners = [
                    Point::new(d.xmin, d.ymin),
                    Point::new(d.xmax, d.ymin),
                    Point::new(d.xmax, d.ymax),
                    Point::new(d.xmin, d.ymax),
                ]
                .map(|p| to_image.apply(p * s));
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for c in corners {
                    x0 = x0.min(c.x);
                    y0 = y0.min(c.y);
                    x1 = x1.max(c.x);
                    y1 = y1.max(c.y);
                }
                Detection {
                    score: d.score,
                    bbox: Rect::new(x0, y0, x1 - x0, y1 - y0),
                    keypoints: d.keypoints.map(|p| to_image.apply(p * s)),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_counts_and_centres_match_mediapipe() {
        let short = AnchorOptions::short_range().anchors();
        assert_eq!(short.len(), 896);
        // Layer 0: 16x16 cells x 2 anchors; first centre (0.5/16, 0.5/16).
        assert_eq!(short[0], Point::new(0.5 / 16.0, 0.5 / 16.0));
        assert_eq!(short[1], short[0]);
        assert_eq!(short[2], Point::new(1.5 / 16.0, 0.5 / 16.0));
        // Layers 1-3 (stride 16) merge into 8x8 cells x 6 anchors.
        assert_eq!(short[512], Point::new(0.5 / 8.0, 0.5 / 8.0));
        assert_eq!(short[517], short[512]);
        assert_eq!(short[518], Point::new(1.5 / 8.0, 0.5 / 8.0));
        assert_eq!(short[895], Point::new(7.5 / 8.0, 7.5 / 8.0));
        let full = AnchorOptions::full_range().anchors();
        assert_eq!(full.len(), 2304);
        assert_eq!(full[1], Point::new(1.5 / 48.0, 0.5 / 48.0));
        assert_eq!(full[48], Point::new(0.5 / 48.0, 1.5 / 48.0));
        assert!(AnchorOptions::for_input_size(100).is_none());
    }

    fn raw_box(dx: f32, dy: f32, w: f32, h: f32) -> [f32; 16] {
        let mut r = [0.0; 16];
        r[0] = dx;
        r[1] = dy;
        r[2] = w;
        r[3] = h;
        for k in 0..6 {
            r[4 + 2 * k] = dx + k as f32;
            r[5 + 2 * k] = dy - k as f32;
        }
        r
    }

    #[test]
    fn decodes_boxes_keypoints_and_scores() {
        let anchors = [Point::new(0.5, 0.5), Point::new(0.25, 0.75)];
        let mut regressors = Vec::new();
        regressors.extend(raw_box(12.8, -6.4, 25.6, 51.2));
        regressors.extend(raw_box(0.0, 0.0, -1.0, 10.0));
        let decoded = decode(&regressors, &[0.0, 100.0], &anchors, 128.0, 0.5);
        // The second box has a negative width and is dropped; the first has score 0.5 (kept).
        assert_eq!(decoded.len(), 1);
        let d = &decoded[0];
        assert!((d.score - 0.5).abs() < 1e-6);
        assert!((d.xmin - (0.6 - 0.1)).abs() < 1e-6 && (d.xmax - 0.7).abs() < 1e-6);
        assert!((d.ymin - (0.45 - 0.2)).abs() < 1e-6 && (d.ymax - 0.65).abs() < 1e-6);
        assert!((d.keypoints[1].x - (0.5 + 13.8 / 128.0)).abs() < 1e-6);
        assert!((d.keypoints[1].y - (0.5 - 7.4 / 128.0)).abs() < 1e-6);
        // Score clamping keeps extreme logits finite.
        let decoded = decode(&regressors[..16], &[1e9], &anchors[..1], 128.0, 0.5);
        assert!((decoded[0].score - 1.0).abs() < 1e-6);
        assert!(decode(&regressors[..16], &[-3.0], &anchors[..1], 128.0, 0.5).is_empty());
    }

    fn det(score: f32, x: f32, y: f32, size: f32) -> TensorDetection {
        TensorDetection {
            score,
            xmin: x,
            ymin: y,
            xmax: x + size,
            ymax: y + size,
            keypoints: [Point::new(x, y); 6],
        }
    }

    #[test]
    fn weighted_nms_merges_overlaps_and_keeps_the_top_score() {
        let dets = vec![
            det(0.6, 0.12, 0.10, 0.2),
            det(0.9, 0.10, 0.10, 0.2),
            det(0.8, 0.60, 0.60, 0.2),
        ];
        let out = weighted_nms(dets, 0.3);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].score, 0.9);
        // Weighted xmin = (0.10 * 0.9 + 0.12 * 0.6) / 1.5 = 0.108.
        assert!((out[0].xmin - 0.108).abs() < 1e-6);
        assert!((out[0].keypoints[0].x - 0.108).abs() < 1e-6);
        assert_eq!(out[1].score, 0.8);
        assert!((out[1].xmin - 0.6).abs() < 1e-6);
        // Non-overlapping detections pass through unchanged.
        let out = weighted_nms(vec![det(0.7, 0.0, 0.0, 0.1), det(0.9, 0.5, 0.5, 0.1)], 0.3);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].score, 0.9);
        assert!(weighted_nms(Vec::new(), 0.3).is_empty());
    }

    #[test]
    fn landmark_roi_follows_the_eye_line() {
        let d = Detection {
            score: 0.9,
            bbox: Rect::new(100.0, 100.0, 80.0, 80.0),
            keypoints: [
                Point::new(120.0, 130.0),
                Point::new(160.0, 140.0),
                Point::default(),
                Point::default(),
                Point::default(),
                Point::default(),
            ],
        };
        let roi = d.landmark_roi();
        assert_eq!(roi.center, Point::new(140.0, 140.0));
        assert_eq!((roi.width, roi.height), (120.0, 120.0));
        // The image-right eye is lower: the face is rotated clockwise, rotation > 0.
        assert!((roi.rotation - (10.0f32).atan2(40.0)).abs() < 1e-6);
        assert!(
            (normalize_radians(3.5 * std::f32::consts::PI) + 0.5 * std::f32::consts::PI).abs()
                < 1e-5
        );
        assert!((d.inter_eye_distance() - 1700.0f32.sqrt()).abs() < 1e-4);
    }
}
