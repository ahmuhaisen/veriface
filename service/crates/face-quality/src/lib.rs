//! Selfie and portrait measurements: pose fit, geometry, illumination, sharpness,
//! segmentation-based checks, output crop.
//!
//! Owner: vision agent (see AGENTS.md). Design: docs/architecture.md section 3.1 (selfie) and
//! section 3.2 step 5 (portrait).
//!
//! The crate produces [`face_core::Measurements`] (a value or an undetermined cause per check) and
//! the face geometry; `face-core` turns them into statuses and verdicts.
//!
//! ```no_run
//! # use face_quality::{Analyzer, AnalyzerConfig, AnalysisOptions};
//! # fn run(runtime: &face_models::Runtime, image: &face_imaging::RgbImage) -> Result<(), face_quality::QualityError> {
//! let analyzer = Analyzer::new(runtime, AnalyzerConfig::default())?;
//! let analysis = analyzer.analyze_selfie(image, &AnalysisOptions {
//!     min_inter_eye_distance_ratio: 0.02, // profile.faceCounting
//!     compression_quality: Some(87.0),    // from codec::Decoded::jpeg_quality
//!     checks: None,                       // or the profile's enabled checks
//! })?;
//! // analysis.measurements -> face_core::evaluate(...)
//! // analysis.face -> API `face`; analysis.output(image, style, w, h) -> output image
//! # Ok(()) }
//! ```
//!
//! Pipeline (selfie): full-range detection (counting, primary = largest eye distance) ->
//! landmarks (short-range ROI on the whole image as MediaPipe's FaceLandmarker does, else on a
//! square crop of 2.2 box widths, else the detection's own ROI) -> blendshapes -> pose
//! (MediaPipe's geometry pipeline + the pinned decomposition, [`pose`]) -> segmentation on the
//! binding ROI ([`face_models::segmentation_roi`]) -> measurements ([`measure`]).

mod analyzer;
mod error;
pub mod geometry;
mod linalg;
pub mod measure;
pub mod output;
pub mod pose;

pub use analyzer::{
    Analysis, AnalysisOptions, Analyzer, AnalyzerConfig, FALLBACK_CROP_BOX_WIDTHS, LandmarkSource,
    subject_checks,
};
pub use error::QualityError;
pub use geometry::{CrownSource, FaceFrame, FaceGeometry, FaceHint, HeadGeometry, head_geometry};
pub use output::{output_rect, render_output};
pub use pose::{
    CanonicalFace, PITCH_OFFSET, PitchOffset, Pose, PoseEstimator, PoseFit, euler_zyx,
    pose_from_euler, rotation_from_pose,
};
