//! ONNX Runtime session management, model manifest verification, and MediaPipe/AuraFace model
//! wrappers.
//!
//! Owner: vision agent (see AGENTS.md). Design: docs/architecture.md sections 2 and 5.
//!
//! # Start-up
//!
//! ```no_run
//! use std::path::Path;
//! use face_models::{Runtime, RuntimeOptions, init_onnxruntime};
//!
//! # fn main() -> Result<(), face_models::ModelError> {
//! // 1. Load Microsoft's onnxruntime library once per process (optionally pin its SHA-256).
//! init_onnxruntime(Path::new("third_party/onnxruntime/1.28.2/win-x64/onnxruntime.dll"), None)?;
//! // 2. Verify models/manifest.json (size + SHA-256 of every file, review status) and create
//! //    one ORT session per model (CPU EP, graph optimisations on, 1 intra-op thread).
//! let runtime = Runtime::load(Path::new("models"), &RuntimeOptions::default())?;
//! // 3. One inference per model before reporting ready.
//! runtime.warm_up()?;
//! # Ok(()) }
//! ```
//!
//! `Runtime` is `Send + Sync`; share it (e.g. in an `Arc`). Sessions are shared too: one ORT
//! session per model by default, used by one inference at a time
//! ([`RuntimeOptions::sessions_per_model`] raises the per-model concurrency).
//!
//! # Running a model directly (e.g. the AuraFace embedder)
//!
//! ```no_run
//! # use face_models::{ImageInputSpec, Runtime, ModelError};
//! # fn embed(runtime: &Runtime, aligned_112: &face_imaging::RgbImage) -> Result<Vec<f32>, ModelError> {
//! let session = runtime.session("auraface-v1")?;
//! // Size, layout (NCHW), colour order (RGB) and normalisation ((x-127.5)/127.5) come from the
//! // manifest `io` spec of the model's first input.
//! let spec = ImageInputSpec::from_tensor_spec(session.id(), &session.entry().io.inputs[0], None)?;
//! let input = spec.to_tensor(aligned_112)?; // [1, 3, 112, 112]; to_batch for N images
//! let outputs = session.run(&[input])?; // every output, graph order, as owned f32 tensors
//! let raw = outputs[0].data().to_vec(); // 512 values, not normalised
//! # Ok(raw) }
//! ```
//!
//! [`ModelSession::run`] validates input count and fixed dimensions; [`ModelSession::outputs`]
//! lists output names and shapes; [`ModelSession::entry`] is the manifest entry (licence, review
//! status, notes).
//!
//! # Wrappers
//!
//! Each wrapper follows the manifest `io` / `notes` and MediaPipe's C++ graphs (Tasks) exactly:
//! - [`FaceDetector`]: BlazeFace short-range (128) and full-range (192); letterbox, SSD anchors,
//!   decoding, weighted NMS, projection.
//! - [`FaceLandmarker`]: 478 landmarks (x, y, z) from a rotated ROI ([`Detection::landmark_roi`])
//!   plus the presence score.
//! - [`Blendshapes`]: 52 coefficients from the 146-landmark subset.
//! - [`Segmenter`]: 6-class category mask over a pixel ROI ([`segmentation_roi`]).
//!
//! Coordinates are pixels of the image passed in, with MediaPipe's numeric conventions
//! (`x_px = x_norm * width`), so they compare directly with `testdata/golden`.

mod blendshapes;
mod detector;
mod error;
mod landmarks;
mod manifest;
mod runtime;
mod segmenter;
mod tensor;
#[cfg(feature = "test-support")]
pub mod test_support;

pub use blendshapes::{BLENDSHAPE_LANDMARKS, Blendshape, BlendshapeScores, Blendshapes};
pub use detector::{
    AnchorOptions, Detection, DetectorConfig, FaceDetector, TensorDetection, decode, keypoint,
    landmark_roi, normalize_radians, weighted_nms,
};
pub use error::ModelError;
pub use landmarks::{
    FaceLandmarker, FaceLandmarks, NUM_LANDMARKS, Point3, mesh, project_landmarks,
};
pub use manifest::{
    AssetEntry, ColorOrder, CommercialUse, Conversion, DType, Dim, IoSpec, Licence, Manifest,
    ModelEntry, ModelRole, Precision, Review, ReviewStatus, TensorSpec, TrainingData, Upstream,
    VerifiedFiles, sha256_file, sha256_hex,
};
pub use runtime::{
    IoInfo, ModelSession, ONNXRUNTIME_VERSION, OrtInfo, Runtime, RuntimeOptions,
    bundled_onnxruntime_path, init_onnxruntime, onnxruntime_info,
};
pub use segmenter::{
    NUM_CLASSES, SegmentClass, Segmentation, Segmenter, category_mask, segmentation_roi,
};
pub use tensor::{ImageInputSpec, Layout, Normalization, TensorF32};

/// Manifest ids of the v1 models (docs/architecture.md section 2).
pub mod ids {
    pub const DETECTOR_SHORT: &str = "mp-face-detector-short";
    pub const DETECTOR_FULL: &str = "mp-face-detector-full";
    pub const LANDMARKER: &str = "mp-face-landmarker";
    pub const BLENDSHAPES: &str = "mp-face-blendshapes";
    pub const SEGMENTER: &str = "mp-selfie-multiclass";
    pub const EMBEDDER: &str = "auraface-v1";
    /// Asset: canonical face geometry for the pose fit.
    pub const CANONICAL_FACE: &str = "mp-canonical-face";
}
