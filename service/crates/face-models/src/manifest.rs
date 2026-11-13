//! `models/manifest.json` (schema: `contracts/model-manifest.schema.json`): typed form, structural
//! validation, integrity verification (size + SHA-256) and lookup by id and role.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::ModelError;

/// The parsed manifest.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    pub manifest_version: u32,
    pub generated_at: String,
    #[serde(default)]
    pub generator: Option<String>,
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub assets: Vec<AssetEntry>,
}

/// What a model is used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelRole {
    FaceDetector,
    FaceLandmarker,
    FaceBlendshapes,
    Segmenter,
    FaceEmbedder,
}

impl ModelRole {
    /// The manifest string, e.g. `face-detector`.
    pub fn as_str(self) -> &'static str {
        match self {
            ModelRole::FaceDetector => "face-detector",
            ModelRole::FaceLandmarker => "face-landmarker",
            ModelRole::FaceBlendshapes => "face-blendshapes",
            ModelRole::Segmenter => "segmenter",
            ModelRole::FaceEmbedder => "face-embedder",
        }
    }
}

/// Weight precision of the stored model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Precision {
    Fp32,
    Fp16,
    Int8,
}

/// One model file.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelEntry {
    pub id: String,
    pub role: ModelRole,
    #[serde(default)]
    pub variant: Option<String>,
    /// Path relative to the models directory.
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
    pub format: String,
    #[serde(default)]
    pub opset: Option<i64>,
    pub precision: Precision,
    pub upstream: Upstream,
    #[serde(default)]
    pub conversion: Option<Conversion>,
    pub licence: Licence,
    pub review: Review,
    pub io: IoSpec,
    #[serde(default)]
    pub notes: Option<String>,
}

/// A non-model data file (e.g. the canonical face geometry).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetEntry {
    pub id: String,
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
    pub upstream: Upstream,
    pub licence: Licence,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Upstream {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    pub url: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Conversion {
    pub tool: String,
    pub tool_version: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub fidelity: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Licence {
    pub spdx: String,
    pub url: String,
    pub training_data: TrainingData,
    #[serde(default)]
    pub attribution: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrainingData {
    pub statement: String,
    pub commercial_use: CommercialUse,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommercialUse {
    Allowed,
    Unclear,
    Prohibited,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub status: ReviewStatus,
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// Legal / licence review status. `rejected` models are refused; `pending` ones load with a
/// warning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewStatus {
    Approved,
    Pending,
    Rejected,
}

impl ReviewStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewStatus::Approved => "approved",
            ReviewStatus::Pending => "pending",
            ReviewStatus::Rejected => "rejected",
        }
    }
}

/// Model inputs and outputs as the lab documented them.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IoSpec {
    pub inputs: Vec<TensorSpec>,
    pub outputs: Vec<TensorSpec>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TensorSpec {
    pub name: String,
    pub dtype: DType,
    pub shape: Vec<Dim>,
    /// e.g. `NHWC`, `NCHW`.
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub color_order: Option<ColorOrder>,
    /// Expected input normalisation, e.g. `(x-127.5)/127.5`, `x/255`, `[-1,1]`.
    #[serde(default)]
    pub value_range: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DType {
    Float32,
    Float16,
    Int8,
    Uint8,
    Int32,
    Int64,
}

/// A tensor dimension: fixed, or symbolic (e.g. `"N"`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Dim {
    Fixed(i64),
    Symbolic(String),
}

impl Dim {
    pub fn fixed(&self) -> Option<i64> {
        match self {
            Dim::Fixed(v) => Some(*v),
            Dim::Symbolic(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum ColorOrder {
    #[serde(rename = "RGB")]
    Rgb,
    #[serde(rename = "BGR")]
    Bgr,
    #[serde(rename = "GRAY")]
    Gray,
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_valid_id(s: &str) -> bool {
    let b = s.as_bytes();
    (2..=64).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// A manifest `file` must be a plain relative path inside the models directory.
fn safe_relative(file: &str) -> bool {
    let p = Path::new(file);
    !file.is_empty()
        && p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

impl Manifest {
    /// Parse and structurally validate a manifest (everything the JSON schema says, plus unique
    /// ids and safe relative file paths).
    pub fn from_json(json: &str, path: &Path) -> Result<Manifest, ModelError> {
        let err = |reason: String| ModelError::Manifest {
            path: path.to_path_buf(),
            reason,
        };
        let m: Manifest = serde_json::from_str(json).map_err(|e| err(e.to_string()))?;
        let mut problems = Vec::new();
        if m.manifest_version != 1 {
            problems.push(format!(
                "manifestVersion {} is not supported (expected 1)",
                m.manifest_version
            ));
        }
        if m.models.is_empty() {
            problems.push("models must not be empty".to_owned());
        }
        let mut ids = HashSet::new();
        let entries = m
            .models
            .iter()
            .map(|e| (&e.id, &e.file, &e.sha256, e.bytes))
            .chain(
                m.assets
                    .iter()
                    .map(|a| (&a.id, &a.file, &a.sha256, a.bytes)),
            );
        for (id, file, sha, bytes) in entries {
            if !is_valid_id(id) {
                problems.push(format!(
                    "id `{id}` does not match ^[a-z0-9][a-z0-9-]{{1,63}}$"
                ));
            }
            if !ids.insert(id.as_str()) {
                problems.push(format!("duplicate id `{id}`"));
            }
            if !is_sha256(sha) {
                problems.push(format!("`{id}`: sha256 must be 64 lowercase hex digits"));
            }
            if !safe_relative(file) {
                problems.push(format!(
                    "`{id}`: file `{file}` must be a relative path inside models/"
                ));
            }
            if bytes == 0 {
                problems.push(format!("`{id}`: bytes must be >= 1"));
            }
        }
        for e in &m.models {
            if e.format != "onnx" {
                problems.push(format!("`{}`: format must be `onnx`", e.id));
            }
            match &e.upstream.sha256 {
                Some(s) if is_sha256(s) => {}
                _ => problems.push(format!(
                    "`{}`: upstream.sha256 is required (64 lowercase hex digits)",
                    e.id
                )),
            }
        }
        if problems.is_empty() {
            Ok(m)
        } else {
            Err(err(problems.join("; ")))
        }
    }

    /// Read and validate `<models_dir>/manifest.json`.
    pub fn load(models_dir: &Path) -> Result<Manifest, ModelError> {
        let path = models_dir.join("manifest.json");
        let json = std::fs::read_to_string(&path).map_err(|source| ModelError::Io {
            path: path.clone(),
            source,
        })?;
        Manifest::from_json(&json, &path)
    }

    pub fn model(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn models_with_role(&self, role: ModelRole) -> impl Iterator<Item = &ModelEntry> {
        self.models.iter().filter(move |m| m.role == role)
    }

    pub fn asset(&self, id: &str) -> Option<&AssetEntry> {
        self.assets.iter().find(|a| a.id == id)
    }

    /// Verify files against the manifest and read them into memory.
    ///
    /// - Models: those in `only` (all when `None`). A `rejected` review status is an error; a
    ///   `pending` one is logged as a warning.
    /// - Assets: always all of them.
    ///
    /// Every file's size and SHA-256 must match. The returned bytes are the verified ones, so
    /// sessions are created from exactly what was hashed.
    pub fn verify(
        &self,
        models_dir: &Path,
        only: Option<&[String]>,
    ) -> Result<VerifiedFiles, ModelError> {
        if let Some(ids) = only {
            for id in ids {
                if self.model(id).is_none() {
                    return Err(ModelError::Unknown(id.clone()));
                }
            }
        }
        let mut models = BTreeMap::new();
        for entry in &self.models {
            if only.is_some_and(|ids| !ids.contains(&entry.id)) {
                continue;
            }
            match entry.review.status {
                ReviewStatus::Rejected => return Err(ModelError::Rejected(entry.id.clone())),
                ReviewStatus::Pending => tracing::warn!(
                    model = %entry.id,
                    "model review status is pending; legal review is required before general availability"
                ),
                ReviewStatus::Approved => {}
            }
            let bytes = read_verified(
                models_dir,
                &entry.id,
                &entry.file,
                entry.bytes,
                &entry.sha256,
            )?;
            models.insert(entry.id.clone(), bytes);
        }
        let mut assets = BTreeMap::new();
        for asset in &self.assets {
            let bytes = read_verified(
                models_dir,
                &asset.id,
                &asset.file,
                asset.bytes,
                &asset.sha256,
            )?;
            assets.insert(asset.id.clone(), Arc::<[u8]>::from(bytes));
        }
        Ok(VerifiedFiles { models, assets })
    }
}

/// Files that passed [`Manifest::verify`].
#[derive(Debug)]
pub struct VerifiedFiles {
    /// Model id to file bytes.
    pub models: BTreeMap<String, Vec<u8>>,
    /// Asset id to file bytes.
    pub assets: BTreeMap<String, Arc<[u8]>>,
}

/// Lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lowercase hex SHA-256 of a file, streamed.
pub fn sha256_file(path: &Path) -> Result<String, ModelError> {
    use std::io::Read;
    let io = |source| ModelError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf).map_err(io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn read_verified(
    models_dir: &Path,
    id: &str,
    file: &str,
    bytes: u64,
    sha256: &str,
) -> Result<Vec<u8>, ModelError> {
    let path: PathBuf = models_dir.join(file);
    let data = std::fs::read(&path).map_err(|source| ModelError::Io {
        path: path.clone(),
        source,
    })?;
    if data.len() as u64 != bytes {
        return Err(ModelError::Integrity {
            id: id.to_owned(),
            path,
            reason: format!("size {} bytes, manifest says {bytes}", data.len()),
        });
    }
    let actual = sha256_hex(&data);
    if actual != sha256 {
        return Err(ModelError::Integrity {
            id: id.to_owned(),
            path,
            reason: format!("SHA-256 {actual} does not match the manifest ({sha256})"),
        });
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json(sha: &str, bytes: u64, status: &str) -> String {
        format!(
            r#"{{
  "manifestVersion": 1,
  "generatedAt": "2026-10-01T00:00:00Z",
  "models": [{{
    "id": "test-model", "role": "face-detector", "variant": "short-range",
    "file": "test.onnx", "sha256": "{sha}", "bytes": {bytes},
    "format": "onnx", "opset": 17, "precision": "fp32",
    "upstream": {{ "name": "u", "url": "https://example.com/u", "sha256": "{sha}" }},
    "licence": {{ "spdx": "Apache-2.0", "url": "https://example.com/l",
                 "trainingData": {{ "statement": "s", "commercialUse": "allowed" }} }},
    "review": {{ "status": "{status}" }},
    "io": {{ "inputs": [{{ "name": "input", "dtype": "float32", "shape": ["N", 3, 4, 4],
                          "layout": "NCHW", "colorOrder": "RGB", "valueRange": "x/255" }}],
             "outputs": [{{ "name": "out", "dtype": "float32", "shape": [1, 2] }}] }}
  }}],
  "assets": [{{
    "id": "test-asset", "file": "asset.json", "sha256": "{sha}", "bytes": {bytes},
    "upstream": {{ "name": "u", "url": "https://example.com/u" }},
    "licence": {{ "spdx": "Apache-2.0", "url": "https://example.com/l",
                 "trainingData": {{ "statement": "s", "commercialUse": "allowed" }} }}
  }}]
}}"#
        )
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "face-models-manifest-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_validates_and_verifies_files() {
        let dir = temp_dir("ok");
        let data = b"model bytes";
        std::fs::write(dir.join("test.onnx"), data).unwrap();
        std::fs::write(dir.join("asset.json"), data).unwrap();
        let sha = sha256_hex(data);
        let json = manifest_json(&sha, data.len() as u64, "approved");
        let m = Manifest::from_json(&json, Path::new("manifest.json")).unwrap();
        assert_eq!(m.model("test-model").unwrap().role, ModelRole::FaceDetector);
        assert_eq!(m.models_with_role(ModelRole::FaceDetector).count(), 1);
        assert_eq!(
            m.models[0].io.inputs[0].shape,
            vec![
                Dim::Symbolic("N".into()),
                Dim::Fixed(3),
                Dim::Fixed(4),
                Dim::Fixed(4)
            ]
        );
        let v = m.verify(&dir, None).unwrap();
        assert_eq!(v.models["test-model"], data);
        assert_eq!(&*v.assets["test-asset"], data);
        assert_eq!(sha256_file(&dir.join("test.onnx")).unwrap(), sha);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_tampered_rejected_and_unsafe_entries() {
        let dir = temp_dir("bad");
        std::fs::write(dir.join("test.onnx"), b"tampered!!!").unwrap();
        std::fs::write(dir.join("asset.json"), b"model bytes").unwrap();
        let sha = sha256_hex(b"model bytes");
        let m = Manifest::from_json(&manifest_json(&sha, 11, "approved"), Path::new("m")).unwrap();
        assert!(matches!(
            m.verify(&dir, None),
            Err(ModelError::Integrity { .. })
        ));
        let rejected =
            Manifest::from_json(&manifest_json(&sha, 11, "rejected"), Path::new("m")).unwrap();
        assert!(matches!(
            rejected.verify(&dir, None),
            Err(ModelError::Rejected(_))
        ));
        assert!(matches!(
            m.verify(&dir, Some(&["nope".to_owned()])),
            Err(ModelError::Unknown(_))
        ));
        // Structural problems are reported at parse time.
        let bad_sha = manifest_json("ABC", 11, "approved");
        assert!(Manifest::from_json(&bad_sha, Path::new("m")).is_err());
        let escape = manifest_json(&sha, 11, "approved").replace("test.onnx", "../escape.onnx");
        assert!(Manifest::from_json(&escape, Path::new("m")).is_err());
        let unknown_field = manifest_json(&sha, 11, "approved").replace("\"opset\"", "\"opsett\"");
        assert!(Manifest::from_json(&unknown_field, Path::new("m")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
