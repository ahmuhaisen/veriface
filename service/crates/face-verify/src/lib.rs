//! AuraFace embedding with five-point alignment, similarity, risk checks and verification
//! assembly (docs/architecture.md section 3.3). Owner: verification agent (see AGENTS.md).
//!
//! - [`align`]: Umeyama similarity fit of the pinned five points to the 112x112 ArcFace
//!   template, warped like OpenCV `warpAffine` (`face_imaging::transform`).
//! - [`embed`]: the [`Embedder`] seam and the AuraFace embedder over a [`TensorRuntime`]
//!   (implemented for `face_models::ModelSession`), manifest-driven preprocessing
//!   (`face_models::ImageInputSpec`), L2 normalisation.
//! - [`risk`]: `PROBE_REFERENCE_DUPLICATE`, `PROBE_DOCUMENT_PRESENT`, `PROBE_SCREEN_ARTIFACTS`.
//! - [`verify()`]: evaluates the probe, reference and risk assessments with `face_core::evaluate`
//!   and applies `face_core::judge`.
//!
//! Embeddings are never logged or stored; [`Embedding`]'s `Debug` prints only its length.

pub mod align;
pub mod embed;
pub mod risk;
mod verify;

pub use align::{ALIGNED_SIZE, ARCFACE_TEMPLATE, AlignedFace, align_face, umeyama};
pub use embed::{
    AURAFACE_DIM, AURAFACE_OUTPUT, AURAFACE_V1, AuraFaceEmbedder, Embedder, Embedding,
    TensorRuntime,
};
pub use verify::{
    Subject, Verification, VerifyError, probe_measurements, verify, verify_with_probe_risk,
};

#[cfg(test)]
mod tests;
