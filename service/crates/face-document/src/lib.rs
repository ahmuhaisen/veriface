//! Document rectification, orientation search, portrait selection and document measurements
//! (docs/architecture.md section 3.2). Owner: verification agent (see AGENTS.md).
//!
//! [`extract_portrait`] runs the pipeline on a reference image:
//! 1. **Rectify** ([`rectify`]): find the document outline on a downscaled copy and map it to a
//!    rectangle with a homography, sampled from the full-resolution input and never upscaled.
//!    The outline confidence is `DOCUMENT_DETECTED`. Skipped for [`ReferenceKind::Portrait`].
//! 2. **Orientation**: run the face finder on the four 90-degree rotations and keep the rotation
//!    with the strongest large face.
//! 3. **Portrait selection**: the face with the largest inter-eye distance; smaller "ghost"
//!    portraits are counted (`PORTRAIT_FOUND`) but not selected.
//! 4. **Portrait analysis**: an upright, full-resolution crop around the portrait goes to the
//!    [`PortraitAnalyzer`] (face-quality), which returns the `portrait`-subject measurements.
//! 5. **Output**: a `face-crop` image that is never upscaled; the caller encodes it.
//!
//! The vision models are reached through two seams, [`FaceFinder`] and [`PortraitAnalyzer`].
//! This crate produces measurements only; `face_core::evaluate` decides statuses.

mod face;
mod orientation;
mod portrait;
pub mod rectify;

#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use face::{
    BoxError, DetectedFace, FaceAnalysis, FaceFinder, FaceGeometry, PortraitAnalyzer, Pose,
};
pub use orientation::{LARGE_FACE_RATIO, Rotation};
pub use portrait::{
    ANALYSIS_CROP_IED, DocumentInfo, DocumentPortrait, FACE_CROP_EYE_LINE, FACE_CROP_HEIGHT_IED,
    MAX_DOCUMENT_IED_RATIO, OutputSize, Portrait, ProcessedAs, ReferenceKind, extract_portrait,
};

/// Why the document pipeline failed. "No document" and "no portrait" are not errors: they are
/// reported as `DOCUMENT_DETECTED` and `PORTRAIT_FOUND`.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("face finder failed: {0}")]
    FaceFinder(#[source] BoxError),
    #[error("portrait analysis failed: {0}")]
    PortraitAnalyzer(#[source] BoxError),
}

#[cfg(test)]
mod fixture_tests;
#[cfg(test)]
mod tests;
