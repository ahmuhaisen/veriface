//! Errors of the model layer.

use std::path::PathBuf;

/// Anything that can go wrong loading, verifying or running a model.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// The ONNX Runtime library could not be loaded or does not match what is expected.
    #[error("ONNX Runtime library {path}: {reason}")]
    OrtLoad { path: PathBuf, reason: String },

    /// A model API was used before [`crate::init_onnxruntime`].
    #[error("ONNX Runtime is not initialised; call face_models::init_onnxruntime first")]
    OrtNotInitialised,

    /// `manifest.json` is unreadable, invalid, or inconsistent with the schema.
    #[error("model manifest {path}: {reason}")]
    Manifest { path: PathBuf, reason: String },

    /// A model or asset file does not match the manifest.
    #[error("`{id}` ({path}): {reason}")]
    Integrity {
        id: String,
        path: PathBuf,
        reason: String,
    },

    /// The manifest marks this model as rejected by review; it must not be loaded.
    #[error("model `{0}` has review status `rejected` and is refused")]
    Rejected(String),

    /// I/O error on a file the model layer needs.
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// No model or asset with this id is loaded.
    #[error("unknown model or asset `{0}`")]
    Unknown(String),

    /// The model's graph or manifest `io` spec does not fit what the wrapper expects.
    #[error("model `{id}`: {reason}")]
    Spec { id: String, reason: String },

    /// ONNX Runtime failed to create a session or run inference.
    #[error("model `{id}`: ONNX Runtime error: {reason}")]
    Runtime { id: String, reason: String },

    /// The caller passed something the model cannot take.
    #[error("invalid input: {0}")]
    InvalidInput(String),
}
