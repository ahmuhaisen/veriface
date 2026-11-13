//! Errors of the quality analysis. Measurement problems are not errors: they become
//! `undetermined` measurements with a cause. These are failures to run the analysis at all.

/// Why an analysis could not run.
#[derive(Debug, thiserror::Error)]
pub enum QualityError {
    /// A model failed to load or run.
    #[error(transparent)]
    Model(#[from] face_models::ModelError),

    /// A required asset is missing or invalid.
    #[error("asset `{id}`: {reason}")]
    Asset { id: &'static str, reason: String },

    /// The input cannot be analysed (e.g. an empty image).
    #[error("invalid input: {0}")]
    InvalidInput(String),
}
