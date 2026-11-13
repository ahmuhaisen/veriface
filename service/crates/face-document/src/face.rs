//! Face types and the two vision seams this crate (and `face-verify`) depends on.
//!
//! The detector and the portrait analysis belong to the vision crates (`face-models`,
//! `face-quality`). They are reached through [`FaceFinder`] and [`PortraitAnalyzer`] so that the
//! document pipeline does not depend on how they are implemented, and so that tests can use the
//! synthetic fakes in [`crate::testing`].

use face_core::Measurements;
use face_imaging::RgbImage;
use face_imaging::geom::{Point, Rect};

/// Error returned by a vision seam.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A face reported by the face detector, in the coordinates of the image it ran on.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectedFace {
    /// Detection box.
    pub bbox: Rect,
    /// Detection confidence, 0-1.
    pub score: f32,
    /// Eye centres from the detector keypoints 0 and 1: the subject's right eye (image-left)
    /// first, then the subject's left eye (image-right).
    pub eyes: [Point; 2],
}

impl DetectedFace {
    /// Distance between the two eye keypoints.
    pub fn inter_eye_distance(&self) -> f32 {
        self.eyes[0].distance(self.eyes[1])
    }

    /// Midpoint of the two eye keypoints.
    pub fn eye_midpoint(&self) -> Point {
        self.eyes[0].midpoint(self.eyes[1])
    }
}

/// Head pose in degrees, with the sign conventions of `contracts/checks.yaml`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

/// Geometry of an analysed face (what `FaceInfo` in the API carries), in the coordinates of the
/// image that was analysed.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceGeometry {
    pub bbox: Rect,
    /// The pinned five points in ArcFace order (docs/architecture.md section 3.3): subject's right
    /// eye centre (MediaPipe 468), subject's left eye centre (473), nose tip (1), subject's right
    /// mouth corner (61), subject's left mouth corner (291).
    pub landmarks5: [Point; 5],
    /// Distance between the two iris centres, in pixels.
    pub inter_eye_distance: f32,
    pub pose: Option<Pose>,
}

/// What a face analysis returns: measurements for the assessment's checks, and the geometry of
/// the analysed face when landmarks were found.
#[derive(Clone, Debug, Default)]
pub struct FaceAnalysis {
    pub measurements: Measurements,
    pub face: Option<FaceGeometry>,
}

/// Seam over the face detector (BlazeFace full-range in `face-models`).
pub trait FaceFinder {
    /// All faces at or above the detector's confidence threshold, after non-maximum suppression,
    /// in `image` coordinates.
    fn find_faces(&self, image: &RgbImage) -> Result<Vec<DetectedFace>, BoxError>;
}

/// The service's detector. Use the full-range model (`face_models::ids::DETECTOR_FULL`) for
/// documents: it finds small portraits (docs/architecture.md section 2).
impl FaceFinder for face_models::FaceDetector {
    fn find_faces(&self, image: &RgbImage) -> Result<Vec<DetectedFace>, BoxError> {
        Ok(self
            .detect(image)?
            .into_iter()
            .map(|d| DetectedFace {
                bbox: d.bbox,
                score: d.score,
                eyes: [d.right_eye(), d.left_eye()],
            })
            .collect())
    }
}

/// Seam over the portrait analysis in `face-quality` (landmarks, pose, segmentation where the
/// profile needs it, and the `portrait`-subject measurements).
pub trait PortraitAnalyzer {
    /// Analyse the portrait in `crop`, an upright full-resolution crop around `face` (given in
    /// crop coordinates). Returns the `portrait`-subject measurements and the face geometry in
    /// crop coordinates (`face: None` when no landmarks were found).
    fn analyze_portrait(
        &self,
        crop: &RgbImage,
        face: &DetectedFace,
    ) -> Result<FaceAnalysis, BoxError>;
}
