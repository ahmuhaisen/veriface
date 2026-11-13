//! MediaPipe Face Mesh v2 landmarks (478 points incl. irises) and mesh index constants.
//!
//! Pre- and post-processing follow `FaceLandmarksDetectorGraph` (Tasks): the rotated ROI is
//! stretched to 256x256 (no letterbox) with bilinear sampling and a replicated border, normalised
//! per the manifest; landmarks `/ 256` are projected back through the ROI
//! (`LandmarkProjectionCalculator` with image dimensions), z scaled like MediaPipe does;
//! presence = sigmoid(logit).

use std::sync::Arc;

use face_imaging::RgbImage;
use face_imaging::geom::Point;
use face_imaging::transform::{Border, RotatedRect, crop_rotated};

use crate::error::ModelError;
use crate::runtime::ModelSession;
use crate::tensor::{ImageInputSpec, Normalization};

/// Number of landmarks the model returns.
pub const NUM_LANDMARKS: usize = 478;

/// A landmark: `x`, `y` in image pixels; `z` relative depth in the same pixel scale as `x`
/// (MediaPipe's normalised z times the image width; smaller = closer to the camera).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Point3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Point3 {
        Point3 { x, y, z }
    }

    pub fn xy(&self) -> Point {
        Point::new(self.x, self.y)
    }
}

/// Landmark indices. "Right" and "left" are the subject's: the subject's right eye is on the
/// image left in a non-mirrored photo.
pub mod mesh {
    /// Centre of the subject's right iris (image-left eye).
    pub const RIGHT_IRIS_CENTER: usize = 468;
    /// Centre of the subject's left iris (image-right eye).
    pub const LEFT_IRIS_CENTER: usize = 473;
    pub const RIGHT_IRIS_RING: [usize; 4] = [469, 470, 471, 472];
    pub const LEFT_IRIS_RING: [usize; 4] = [474, 475, 476, 477];
    pub const NOSE_TIP: usize = 1;
    /// Subject's right mouth corner (image-left).
    pub const MOUTH_RIGHT: usize = 61;
    /// Subject's left mouth corner (image-right).
    pub const MOUTH_LEFT: usize = 291;
    /// Lowest point of the chin.
    pub const CHIN: usize = 152;
    /// Top of the forehead (around the hairline).
    pub const FOREHEAD_TOP: usize = 10;
    /// Between the eyes, on the nose bridge.
    pub const MIDWAY_BETWEEN_EYES: usize = 168;
    pub const RIGHT_CHEEK: usize = 205;
    pub const LEFT_CHEEK: usize = 425;
    /// Face contour at ear level, image-left (subject's right).
    pub const RIGHT_FACE_EDGE: usize = 234;
    /// Face contour at ear level, image-right (subject's left).
    pub const LEFT_FACE_EDGE: usize = 454;

    /// The five alignment points in ArcFace template order (docs/architecture.md section 3.3):
    /// right eye centre, left eye centre, nose tip, right mouth corner, left mouth corner.
    pub const ALIGNMENT_5: [usize; 5] = [468, 473, 1, 61, 291];

    /// Face outline (forehead, cheeks, jaw, chin), closed polygon in order.
    pub const FACE_OVAL: [usize; 36] = [
        10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377,
        152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234, 127, 162, 21, 54, 103, 67, 109,
    ];

    /// Subject's right eye contour (image-left), closed polygon: outer corner, lower lid, inner
    /// corner, upper lid.
    pub const RIGHT_EYE: [usize; 16] = [
        33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246,
    ];
    /// Subject's left eye contour (image-right), closed polygon.
    pub const LEFT_EYE: [usize; 16] = [
        263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466,
    ];
    /// Subject's right eye: outer corner, inner corner, upper lid centre, lower lid centre.
    pub const RIGHT_EYE_CORNERS_LIDS: [usize; 4] = [33, 133, 159, 145];
    /// Subject's left eye: outer corner, inner corner, upper lid centre, lower lid centre.
    pub const LEFT_EYE_CORNERS_LIDS: [usize; 4] = [263, 362, 386, 374];

    pub const RIGHT_EYEBROW: [usize; 10] = [70, 63, 105, 66, 107, 55, 65, 52, 53, 46];
    pub const LEFT_EYEBROW: [usize; 10] = [300, 293, 334, 296, 336, 285, 295, 282, 283, 276];

    /// Outer lip contour, closed polygon.
    pub const LIPS_OUTER: [usize; 20] = [
        61, 185, 40, 39, 37, 0, 267, 269, 270, 409, 291, 375, 321, 405, 314, 17, 84, 181, 91, 146,
    ];
    /// Inner lip contour, closed polygon.
    pub const LIPS_INNER: [usize; 20] = [
        78, 191, 80, 81, 82, 13, 312, 311, 310, 415, 308, 324, 318, 402, 317, 14, 87, 178, 88, 95,
    ];
    /// Upper / lower inner-lip pairs across the mouth opening (centre and both sides).
    pub const INNER_LIP_PAIRS: [(usize, usize); 3] = [(13, 14), (82, 87), (312, 317)];
}

/// Landmarks of one face.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceLandmarks {
    /// [`NUM_LANDMARKS`] points in the coordinates of the image passed in.
    pub points: Vec<Point3>,
    /// Face presence probability (sigmoid of the presence logit). MediaPipe drops faces below 0.5.
    pub presence: f32,
    /// The ROI the model saw.
    pub roi: RotatedRect,
}

impl FaceLandmarks {
    /// 2-D position of landmark `i`.
    pub fn point(&self, i: usize) -> Point {
        self.points[i].xy()
    }

    /// Distance between the two iris centres (468, 473).
    pub fn inter_eye_distance(&self) -> f32 {
        self.point(mesh::RIGHT_IRIS_CENTER)
            .distance(self.point(mesh::LEFT_IRIS_CENTER))
    }

    /// The five alignment points ([`mesh::ALIGNMENT_5`]).
    pub fn landmarks5(&self) -> [Point; 5] {
        mesh::ALIGNMENT_5.map(|i| self.point(i))
    }

    /// The same landmarks translated by `(dx, dy)` (e.g. from crop to image coordinates).
    pub fn translated(&self, dx: f32, dy: f32) -> FaceLandmarks {
        FaceLandmarks {
            points: self
                .points
                .iter()
                .map(|p| Point3::new(p.x + dx, p.y + dy, p.z))
                .collect(),
            presence: self.presence,
            roi: RotatedRect {
                center: Point::new(self.roi.center.x + dx, self.roi.center.y + dy),
                ..self.roi
            },
        }
    }
}

/// Project raw model landmarks (`3 * NUM_LANDMARKS` values, tensor pixels of a `size` x `size`
/// input) into the coordinates of a `width` x `height` image through `roi`. z follows
/// `LandmarkProjectionCalculator` with `IMAGE_DIMENSIONS`: it is scaled by the length of the ROI's
/// x axis in normalised image coordinates, then expressed in pixels of the image width.
pub fn project_landmarks(
    raw: &[f32],
    size: u32,
    roi: &RotatedRect,
    width: u32,
    height: u32,
) -> Vec<Point3> {
    let to_image = roi.output_to_source(size, size);
    let (cos, sin) = (roi.rotation.cos(), roi.rotation.sin());
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let z_scale_px = roi.width * (cos * cos + (sin * w / h).powi(2)).sqrt() / size as f32;
    raw.as_chunks::<3>()
        .0
        .iter()
        .take(NUM_LANDMARKS)
        .map(|v| {
            let p = to_image.apply(Point::new(v[0], v[1]));
            Point3::new(p.x, p.y, v[2] * z_scale_px)
        })
        .collect()
}

/// The landmark model bound to a session.
#[derive(Debug, Clone)]
pub struct FaceLandmarker {
    session: Arc<ModelSession>,
    spec: ImageInputSpec,
    landmarks: usize,
    presence: usize,
}

impl FaceLandmarker {
    pub fn new(session: Arc<ModelSession>) -> Result<FaceLandmarker, ModelError> {
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
        let spec = ImageInputSpec::from_tensor_spec(&id, input, Some(Normalization::UNIT))?;
        let elems = |shape: &[i64]| shape.iter().map(|d| (*d).max(1)).product::<i64>();
        let landmarks = session
            .output_index("landmarks")
            .or_else(|| {
                session
                    .outputs()
                    .iter()
                    .position(|o| elems(&o.shape) == 3 * NUM_LANDMARKS as i64)
            })
            .ok_or_else(|| bad("no 1434-value landmarks output".to_owned()))?;
        // The presence logit is the first single-value output after the landmarks in MediaPipe's
        // order (`tongue_out` comes last).
        let presence = session
            .output_index("presence_logit")
            .or_else(|| {
                session
                    .outputs()
                    .iter()
                    .position(|o| o.shape.len() == 4 && elems(&o.shape) == 1)
            })
            .ok_or_else(|| bad("no presence output".to_owned()))?;
        Ok(FaceLandmarker {
            session,
            spec,
            landmarks,
            presence,
        })
    }

    /// Input size (256).
    pub fn input_size(&self) -> u32 {
        self.spec.width
    }

    pub fn session(&self) -> &Arc<ModelSession> {
        &self.session
    }

    /// The uint8 tensor image the model sees for `roi` (before normalisation).
    pub fn preprocess(&self, image: &RgbImage, roi: &RotatedRect) -> RgbImage {
        crop_rotated(
            image,
            roi,
            self.spec.width,
            self.spec.height,
            Border::Replicate,
        )
    }

    /// Landmarks of the face in `roi` (usually [`crate::Detection::landmark_roi`]).
    pub fn landmarks(
        &self,
        image: &RgbImage,
        roi: &RotatedRect,
    ) -> Result<FaceLandmarks, ModelError> {
        if image.width() == 0 || image.height() == 0 || roi.width <= 0.0 || roi.height <= 0.0 {
            return Err(ModelError::InvalidInput("empty image or ROI".to_owned()));
        }
        let input = self.spec.to_tensor(&self.preprocess(image, roi))?;
        let outputs = self.session.run(&[input])?;
        let raw = outputs[self.landmarks].data();
        let logit = outputs[self.presence]
            .data()
            .first()
            .copied()
            .unwrap_or(f32::NEG_INFINITY);
        Ok(self.decode(raw, logit, roi, image.width(), image.height()))
    }

    /// Turn raw outputs into [`FaceLandmarks`] for a `width` x `height` image.
    pub fn decode(
        &self,
        raw: &[f32],
        presence_logit: f32,
        roi: &RotatedRect,
        width: u32,
        height: u32,
    ) -> FaceLandmarks {
        FaceLandmarks {
            points: project_landmarks(raw, self.spec.width, roi, width, height),
            presence: 1.0 / (1.0 + (-presence_logit).exp()),
            roi: *roi,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_maps_tensor_corners_and_scales_z_like_mediapipe() {
        let roi = RotatedRect {
            center: Point::new(100.0, 80.0),
            width: 64.0,
            height: 64.0,
            rotation: 0.0,
        };
        let raw = [0.0, 0.0, 256.0, 256.0, 256.0, -128.0];
        let pts = project_landmarks(&raw, 256, &roi, 400, 300);
        assert_eq!(pts.len(), 2);
        assert!((pts[0].x - 68.0).abs() < 1e-4 && (pts[0].y - 48.0).abs() < 1e-4);
        // z scales like x: 256 tensor px over a 64 px ROI -> 64 px.
        assert!((pts[0].z - 64.0).abs() < 1e-4);
        assert!((pts[1].x - 132.0).abs() < 1e-4 && (pts[1].y - 112.0).abs() < 1e-4);
        assert!((pts[1].z + 32.0).abs() < 1e-4);
        // Rotated ROI in a non-square image: MediaPipe measures the ROI x axis in normalised image
        // coordinates, then z is expressed in image-width pixels.
        let r90 = RotatedRect {
            rotation: std::f32::consts::FRAC_PI_2,
            ..roi
        };
        let pts = project_landmarks(&[128.0, 128.0, 256.0], 256, &r90, 400, 200);
        assert!((pts[0].x - 100.0).abs() < 1e-3 && (pts[0].y - 80.0).abs() < 1e-3);
        assert!((pts[0].z - 64.0 * 2.0).abs() < 1e-2);
    }

    #[test]
    fn mesh_constants_are_consistent() {
        assert_eq!(mesh::ALIGNMENT_5, [468, 473, 1, 61, 291]);
        let mut oval = mesh::FACE_OVAL.to_vec();
        oval.sort_unstable();
        oval.dedup();
        assert_eq!(oval.len(), 36);
        assert!(mesh::RIGHT_EYE.contains(&33) && mesh::LEFT_EYE.contains(&263));
        for i in mesh::FACE_OVAL
            .iter()
            .chain(&mesh::RIGHT_EYE)
            .chain(&mesh::LEFT_EYE)
            .chain(&mesh::LIPS_INNER)
            .chain(&mesh::LIPS_OUTER)
        {
            assert!(*i < 468);
        }
    }
}
