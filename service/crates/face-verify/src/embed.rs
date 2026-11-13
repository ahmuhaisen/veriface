//! Face embeddings: the AuraFace embedder over an ONNX Runtime seam, L2 normalisation and cosine
//! similarity.

use std::sync::Arc;

use face_document::BoxError;
use face_imaging::RgbImage;
use face_models::{ImageInputSpec, ModelError, ModelSession, TensorF32};

use crate::align::ALIGNED_SIZE;

/// Manifest id of the AuraFace v1 recognition model (`models/manifest.json`).
pub const AURAFACE_V1: &str = "auraface-v1";
/// Length of an AuraFace embedding.
pub const AURAFACE_DIM: usize = 512;
/// Name of the AuraFace output tensor in the manifest.
pub const AURAFACE_OUTPUT: &str = "embedding";

/// An L2-normalised face embedding. Never logged or stored (AGENTS.md section 2).
#[derive(Clone, PartialEq)]
pub struct Embedding(Vec<f32>);

impl std::fmt::Debug for Embedding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Biometric template: print only its size.
        write!(f, "Embedding({} dims)", self.0.len())
    }
}

impl Embedding {
    /// L2-normalise `raw`. `None` when it is empty, not finite, or all zeros.
    pub fn from_raw(raw: Vec<f32>) -> Option<Embedding> {
        let norm = raw
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum::<f64>()
            .sqrt();
        if raw.is_empty() || !norm.is_finite() || norm <= 1e-12 {
            return None;
        }
        Some(Embedding(
            raw.iter().map(|v| (f64::from(*v) / norm) as f32).collect(),
        ))
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Cosine similarity in [-1, 1] (the dot product of normalised vectors). `None` when the
    /// lengths differ.
    pub fn cosine(&self, other: &Embedding) -> Option<f64> {
        (self.0.len() == other.0.len()).then(|| {
            self.0
                .iter()
                .zip(&other.0)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum::<f64>()
                .clamp(-1.0, 1.0)
        })
    }
}

/// Seam over the face-embedding model.
pub trait Embedder {
    /// Manifest id of the model; must equal `profile.verification.model`.
    fn model_id(&self) -> &str;

    /// Embeddings of 112x112 faces aligned to the ArcFace template ([`crate::align_face`]), in
    /// input order. One model call for the whole batch where the runtime allows it.
    fn embed_batch(&self, aligned: &[&RgbImage]) -> Result<Vec<Embedding>, BoxError>;

    /// Embedding of one aligned face.
    fn embed(&self, aligned: &RgbImage) -> Result<Embedding, BoxError> {
        self.embed_batch(&[aligned])?
            .pop()
            .ok_or_else(|| "the embedder returned no embedding".into())
    }
}

/// Seam over one model session: run the model on its single input tensor and return the
/// embedding output. Implemented for `face_models::ModelSession`; tests use fakes.
pub trait TensorRuntime {
    fn infer(&self, input: TensorF32) -> Result<TensorF32, BoxError>;
}

impl TensorRuntime for ModelSession {
    fn infer(&self, input: TensorF32) -> Result<TensorF32, BoxError> {
        let index = self.output_index(AURAFACE_OUTPUT).unwrap_or(0);
        let mut outputs = ModelSession::run(self, &[input])?;
        if index >= outputs.len() {
            return Err(format!("model `{}` returned no output {index}", self.id()).into());
        }
        Ok(outputs.swap_remove(index))
    }
}

impl<T: TensorRuntime + ?Sized> TensorRuntime for Arc<T> {
    fn infer(&self, input: TensorF32) -> Result<TensorF32, BoxError> {
        (**self).infer(input)
    }
}

/// The AuraFace v1 embedder: input preprocessing from the model's manifest `io` spec (RGB, NCHW,
/// `(x-127.5)/127.5` for `auraface-v1`), one runtime call per batch, L2 normalisation of each
/// 512-d output.
#[derive(Clone, Debug)]
pub struct AuraFaceEmbedder<R> {
    runtime: R,
    spec: ImageInputSpec,
    model_id: String,
}

impl<R: TensorRuntime> AuraFaceEmbedder<R> {
    /// An embedder over `runtime` with the given input spec (normally read from the manifest).
    pub fn new(runtime: R, spec: ImageInputSpec) -> AuraFaceEmbedder<R> {
        AuraFaceEmbedder {
            runtime,
            spec,
            model_id: AURAFACE_V1.to_owned(),
        }
    }
}

impl AuraFaceEmbedder<Arc<ModelSession>> {
    /// The embedder for a loaded session (`runtime.session("auraface-v1")`), with the input spec
    /// from its manifest entry. The input must be 112x112.
    pub fn from_session(session: Arc<ModelSession>) -> Result<Self, ModelError> {
        let input = session
            .entry()
            .io
            .inputs
            .first()
            .ok_or_else(|| ModelError::Spec {
                id: session.id().to_owned(),
                reason: "no input tensor in the manifest".to_owned(),
            })?;
        let spec = ImageInputSpec::from_tensor_spec(session.id(), input, None)?;
        if (spec.width, spec.height) != (ALIGNED_SIZE, ALIGNED_SIZE) {
            return Err(ModelError::Spec {
                id: session.id().to_owned(),
                reason: format!(
                    "the embedder needs a {ALIGNED_SIZE}x{ALIGNED_SIZE} input, the manifest says {}x{}",
                    spec.width, spec.height
                ),
            });
        }
        Ok(AuraFaceEmbedder {
            model_id: session.id().to_owned(),
            runtime: session,
            spec,
        })
    }
}

impl<R: TensorRuntime> Embedder for AuraFaceEmbedder<R> {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn embed_batch(&self, aligned: &[&RgbImage]) -> Result<Vec<Embedding>, BoxError> {
        if aligned.is_empty() {
            return Ok(Vec::new());
        }
        let output = self.runtime.infer(self.spec.to_batch(aligned)?)?;
        let values = output.data();
        if values.len() != aligned.len() * AURAFACE_DIM {
            return Err(format!(
                "expected {} x {AURAFACE_DIM} output values, got {} (shape {:?})",
                aligned.len(),
                values.len(),
                output.shape()
            )
            .into());
        }
        values
            .as_chunks::<AURAFACE_DIM>()
            .0
            .iter()
            .map(|raw| {
                Embedding::from_raw(raw.to_vec())
                    .ok_or_else(|| BoxError::from("embedding is zero or not finite"))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use face_models::{ColorOrder, Layout, Normalization};
    use image::Rgb;
    use std::cell::RefCell;

    #[test]
    fn normalises_and_compares() {
        let a = Embedding::from_raw(vec![3.0, 4.0]).unwrap();
        assert_eq!(a.as_slice(), &[0.6, 0.8]);
        let b = Embedding::from_raw(vec![-4.0, 3.0]).unwrap();
        assert!(a.cosine(&b).unwrap().abs() < 1e-7);
        assert!((a.cosine(&a).unwrap() - 1.0).abs() < 1e-7);
        assert!(Embedding::from_raw(vec![0.0; 4]).is_none());
        assert!(Embedding::from_raw(vec![f32::NAN, 1.0]).is_none());
        let c = Embedding::from_raw(vec![1.0, 0.0, 0.0]).unwrap();
        assert!(a.cosine(&c).is_none());
        assert_eq!(format!("{a:?}"), "Embedding(2 dims)");
    }

    /// The `auraface-v1` input spec from models/manifest.json.
    fn auraface_spec() -> ImageInputSpec {
        ImageInputSpec {
            width: 112,
            height: 112,
            layout: Layout::Nchw,
            color: ColorOrder::Rgb,
            normalization: Normalization::parse("(x-127.5)/127.5").unwrap(),
        }
    }

    /// Records the input it was called with and returns a different vector per batch item.
    struct FakeRuntime(RefCell<Vec<TensorF32>>);

    impl TensorRuntime for FakeRuntime {
        fn infer(&self, input: TensorF32) -> Result<TensorF32, BoxError> {
            let n = input.shape()[0];
            let per = input.len() / n;
            let data: Vec<f32> = (0..n)
                .flat_map(|i| {
                    let mean =
                        input.data()[i * per..(i + 1) * per].iter().sum::<f32>() / per as f32;
                    (0..AURAFACE_DIM).map(move |k| if k == i { 2.0 + mean } else { 0.0 })
                })
                .collect();
            self.0.borrow_mut().push(input);
            Ok(TensorF32::new(vec![n, AURAFACE_DIM], data)?)
        }
    }

    #[test]
    fn auraface_embedder_batches_normalises_and_preprocesses_per_the_manifest() {
        let e = AuraFaceEmbedder::new(FakeRuntime(RefCell::new(Vec::new())), auraface_spec());
        let mut a = RgbImage::from_pixel(112, 112, Rgb([255, 0, 128]));
        a.put_pixel(1, 0, Rgb([0, 255, 127]));
        let b = RgbImage::from_pixel(112, 112, Rgb([10, 10, 200]));
        let out = e.embed_batch(&[&a, &b]).unwrap();
        let calls = e.runtime.0.borrow();
        assert_eq!(calls.len(), 1, "one call per batch");
        let input = &calls[0];
        assert_eq!(input.shape(), &[2, 3, 112, 112]);
        // NCHW, RGB, (x - 127.5) / 127.5: R plane first.
        let plane = 112 * 112;
        let d = input.data();
        assert!((d[0] - 1.0).abs() < 1e-6 && (d[1] - -1.0).abs() < 1e-6);
        assert!((d[plane] - -1.0).abs() < 1e-6 && (d[plane + 1] - 1.0).abs() < 1e-6);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|v| v.len() == AURAFACE_DIM));
        assert!((out[0].as_slice()[0] - 1.0).abs() < 1e-6);
        assert!(out[0].cosine(&out[1]).unwrap().abs() < 1e-9);
        assert_eq!(e.model_id(), AURAFACE_V1);
        drop(calls);
        assert!(e.embed(&RgbImage::new(100, 112)).is_err());
    }
}
