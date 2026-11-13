//! ONNX Runtime loading, model sessions and the [`Runtime`] registry.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::TensorRef;

use crate::error::ModelError;
use crate::manifest::{DType, Manifest, ModelEntry, ModelRole, sha256_file};
use crate::tensor::TensorF32;

/// The ONNX Runtime release this crate is built and tested against (ort 2.0.0-rc.13, C API 1.28).
pub const ONNXRUNTIME_VERSION: &str = "1.28.2";

/// Where `service/scripts/fetch-onnxruntime.*` puts the runtime library for this platform, given
/// the `service/third_party` directory.
pub fn bundled_onnxruntime_path(third_party_dir: &Path) -> PathBuf {
    let base = third_party_dir
        .join("onnxruntime")
        .join(ONNXRUNTIME_VERSION);
    if cfg!(windows) {
        base.join("win-x64").join("onnxruntime.dll")
    } else {
        base.join("linux-x64")
            .join(format!("libonnxruntime.so.{ONNXRUNTIME_VERSION}"))
    }
}

/// The loaded ONNX Runtime library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrtInfo {
    /// Canonical path of the library file.
    pub path: PathBuf,
    /// Version, e.g. `1.28.2` (from the library's build info).
    pub version: String,
    /// SHA-256 of the library file.
    pub sha256: String,
}

static ORT: Mutex<Option<Arc<OrtInfo>>> = Mutex::new(None);

/// Load Microsoft's ONNX Runtime library from `path` and create the process-wide ORT environment
/// (telemetry off). Call once at start-up, before [`Runtime::load`].
///
/// - ort (feature `api-28`) refuses any library older than the 1.28 C API. When the release's
///   `VERSION_NUMBER` file sits next to the library (as `fetch-onnxruntime.*` leaves it) and its
///   `GIT_COMMIT_ID` matches the library's build info, the version must be 1.28.x.
/// - `expected_sha256`, when given, pins the exact file (lowercase or uppercase hex).
/// - A process can load only one ONNX Runtime: calling again with the same file returns the same
///   [`OrtInfo`]; a different file is an error.
///
/// If Windows Code Integrity / Smart App Control blocks the library, the error says the library
/// failed to load. Microsoft's official build is Authenticode-signed.
pub fn init_onnxruntime(
    path: &Path,
    expected_sha256: Option<&str>,
) -> Result<Arc<OrtInfo>, ModelError> {
    let mut state = ORT.lock().unwrap_or_else(|e| e.into_inner());
    let canonical = std::fs::canonicalize(path).map_err(|source| ModelError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if let Some(info) = state.as_ref() {
        return if info.path == canonical {
            Ok(Arc::clone(info))
        } else {
            Err(ModelError::OrtLoad {
                path: canonical,
                reason: format!(
                    "ONNX Runtime is already loaded from {}; a process can load only one",
                    info.path.display()
                ),
            })
        };
    }
    let load_err = |reason: String| ModelError::OrtLoad {
        path: canonical.clone(),
        reason,
    };
    let sha256 = sha256_file(&canonical)?;
    if let Some(expected) = expected_sha256
        && !expected.eq_ignore_ascii_case(&sha256)
    {
        return Err(load_err(format!(
            "SHA-256 {sha256} does not match the pinned {expected}"
        )));
    }
    let builder = ort::init_from(&canonical).map_err(|e| load_err(e.to_string()))?;
    builder
        .with_name("veriface")
        .with_telemetry(false)
        .commit();
    let version = release_version(&canonical, ort::info());
    match &version {
        Some(v) if !v.starts_with("1.28.") => {
            return Err(load_err(format!(
                "version {v} is not supported; ONNX Runtime 1.28.x is required"
            )));
        }
        Some(_) => {}
        None => tracing::warn!(
            path = %canonical.display(),
            "ONNX Runtime version unknown (no matching VERSION_NUMBER next to the library);              the 1.28 C API is required and was accepted"
        ),
    }
    let version = version.unwrap_or_else(|| "unknown".to_owned());
    tracing::info!(path = %canonical.display(), %version, %sha256, "ONNX Runtime loaded");
    let info = Arc::new(OrtInfo {
        path: canonical,
        version,
        sha256,
    });
    *state = Some(Arc::clone(&info));
    Ok(info)
}

/// The loaded ONNX Runtime, if [`init_onnxruntime`] succeeded.
pub fn onnxruntime_info() -> Option<Arc<OrtInfo>> {
    ORT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The release version from the `VERSION_NUMBER` file next to the library, if its `GIT_COMMIT_ID`
/// (when present) matches the commit in the library's build info.
fn release_version(library: &Path, build_info: &str) -> Option<String> {
    let dir = library.parent()?;
    let version = std::fs::read_to_string(dir.join("VERSION_NUMBER"))
        .ok()?
        .trim()
        .to_owned();
    if let Ok(commit) = std::fs::read_to_string(dir.join("GIT_COMMIT_ID")) {
        let built = build_info_commit(build_info)?;
        if !commit.trim().starts_with(built) {
            return None;
        }
    }
    (!version.is_empty()).then_some(version)
}

/// `33ca962823` from `ORT Build Info: git-branch=HEAD, git-commit-id=33ca962823, ...`.
fn build_info_commit(info: &str) -> Option<&str> {
    let start = info.find("git-commit-id=")? + "git-commit-id=".len();
    let rest = &info[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_hexdigit())
        .unwrap_or(rest.len());
    (end >= 7).then(|| &rest[..end])
}

/// Session settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeOptions {
    /// Threads ONNX Runtime may use inside one inference. Default 1: the service parallelises
    /// across requests instead.
    pub intra_op_threads: usize,
    /// ORT sessions per model. Each session runs one inference at a time, so this bounds the
    /// per-model concurrency; every extra session holds another copy of the weights (AuraFace is
    /// ~260 MB). Default 1.
    pub sessions_per_model: usize,
    /// Enable ONNX Runtime graph optimisations (all levels). Default true.
    pub graph_optimizations: bool,
    /// Load only these model ids (default: every model in the manifest). Assets are always
    /// verified.
    pub models: Option<Vec<String>>,
    /// Per-model `intra_op_threads` overrides, by model id (e.g. `auraface-v1` -> 2). Models not
    /// listed use `intra_op_threads`. Default: none.
    pub intra_op_threads_per_model: BTreeMap<String, usize>,
}

impl RuntimeOptions {
    /// Intra-op threads for `model_id` (its override, else the default).
    pub fn intra_op_threads_for(&self, model_id: &str) -> usize {
        self.intra_op_threads_per_model
            .get(model_id)
            .copied()
            .unwrap_or(self.intra_op_threads)
            .max(1)
    }
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        RuntimeOptions {
            intra_op_threads: 1,
            sessions_per_model: 1,
            graph_optimizations: true,
            models: None,
            intra_op_threads_per_model: BTreeMap::new(),
        }
    }
}

/// Name and shape of a model input or output, as the ONNX graph declares it (`-1` = dynamic).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IoInfo {
    pub name: String,
    pub shape: Vec<i64>,
}

/// An ONNX Runtime session (or a small pool of them) for one model. Safe to share across threads:
/// [`ModelSession::run`] takes `&self` and uses a free session from the pool, waiting when all of
/// them are busy.
#[derive(Debug)]
pub struct ModelSession {
    entry: ModelEntry,
    pool: Vec<Mutex<Session>>,
    next: AtomicUsize,
    inputs: Vec<IoInfo>,
    outputs: Vec<IoInfo>,
}

impl ModelSession {
    fn create(
        entry: ModelEntry,
        bytes: &[u8],
        options: &RuntimeOptions,
    ) -> Result<ModelSession, ModelError> {
        let id = entry.id.clone();
        let rt = |reason: String| ModelError::Runtime {
            id: id.clone(),
            reason,
        };
        let level = if options.graph_optimizations {
            GraphOptimizationLevel::All
        } else {
            GraphOptimizationLevel::Disable
        };
        let mut pool = Vec::new();
        for _ in 0..options.sessions_per_model.max(1) {
            let session = Session::builder()
                .map_err(|e| rt(e.to_string()))?
                .with_optimization_level(level)
                .map_err(|e| rt(e.to_string()))?
                .with_intra_threads(options.intra_op_threads_for(&id))
                .map_err(|e| rt(e.to_string()))?
                .with_inter_threads(1)
                .map_err(|e| rt(e.to_string()))?
                .with_execution_providers([ort::ep::CPU::default().build()])
                .map_err(|e| rt(e.to_string()))?
                .commit_from_memory(bytes)
                .map_err(|e| rt(e.to_string()))?;
            pool.push(Mutex::new(session));
        }
        let (inputs, outputs) = {
            let s = pool[0].lock().unwrap_or_else(|e| e.into_inner());
            let describe = |outlets: &[ort::value::Outlet]| -> Result<Vec<IoInfo>, ModelError> {
                outlets
                    .iter()
                    .map(|o| {
                        let shape = o.dtype().tensor_shape().ok_or_else(|| ModelError::Spec {
                            id: id.clone(),
                            reason: format!("`{}` is not a tensor", o.name()),
                        })?;
                        if o.dtype().tensor_type() != Some(ort::value::TensorElementType::Float32) {
                            return Err(ModelError::Spec {
                                id: id.clone(),
                                reason: format!("`{}` is not float32", o.name()),
                            });
                        }
                        Ok(IoInfo {
                            name: o.name().to_owned(),
                            shape: shape.to_vec(),
                        })
                    })
                    .collect()
            };
            (describe(s.inputs())?, describe(s.outputs())?)
        };
        let session = ModelSession {
            entry,
            pool,
            next: AtomicUsize::new(0),
            inputs,
            outputs,
        };
        session.check_against_manifest()?;
        Ok(session)
    }

    /// The graph must have exactly the manifest's inputs and outputs (names, float32, and every
    /// dimension the manifest fixes).
    fn check_against_manifest(&self) -> Result<(), ModelError> {
        let bad = |reason: String| ModelError::Spec {
            id: self.entry.id.clone(),
            reason,
        };
        for (kind, graph, specs) in [
            ("input", &self.inputs, &self.entry.io.inputs),
            ("output", &self.outputs, &self.entry.io.outputs),
        ] {
            if graph.len() != specs.len() {
                return Err(bad(format!(
                    "the graph has {} {kind}s, the manifest lists {}",
                    graph.len(),
                    specs.len()
                )));
            }
            for spec in specs.iter() {
                let Some(g) = graph.iter().find(|g| g.name == spec.name) else {
                    return Err(bad(format!(
                        "manifest {kind} `{}` is not in the graph",
                        spec.name
                    )));
                };
                if spec.dtype != DType::Float32 {
                    return Err(bad(format!("{kind} `{}` must be float32", spec.name)));
                }
                if g.shape.len() != spec.shape.len() {
                    return Err(bad(format!(
                        "{kind} `{}`: graph rank {} vs manifest rank {}",
                        spec.name,
                        g.shape.len(),
                        spec.shape.len()
                    )));
                }
                for (gd, sd) in g.shape.iter().zip(&spec.shape) {
                    if let Some(fixed) = sd.fixed()
                        && *gd >= 0
                        && *gd != fixed
                    {
                        return Err(bad(format!(
                            "{kind} `{}`: graph shape {:?} vs manifest {:?}",
                            spec.name, g.shape, spec.shape
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Manifest id.
    pub fn id(&self) -> &str {
        &self.entry.id
    }

    pub fn role(&self) -> ModelRole {
        self.entry.role
    }

    /// The model's manifest entry (licence, review status, `io` spec, notes...).
    pub fn entry(&self) -> &ModelEntry {
        &self.entry
    }

    /// Graph inputs, in graph order (the order [`ModelSession::run`] expects).
    pub fn inputs(&self) -> &[IoInfo] {
        &self.inputs
    }

    /// Graph outputs, in graph order (the order [`ModelSession::run`] returns).
    pub fn outputs(&self) -> &[IoInfo] {
        &self.outputs
    }

    /// Index of the output named `name`.
    pub fn output_index(&self, name: &str) -> Option<usize> {
        self.outputs.iter().position(|o| o.name == name)
    }

    /// Number of ORT sessions in the pool.
    pub fn pool_size(&self) -> usize {
        self.pool.len()
    }

    fn acquire(&self) -> MutexGuard<'_, Session> {
        let n = self.pool.len();
        let start = self.next.fetch_add(1, Ordering::Relaxed) % n;
        for k in 0..n {
            if let Ok(guard) = self.pool[(start + k) % n].try_lock() {
                return guard;
            }
        }
        self.pool[start].lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run inference. `inputs` are in graph input order and must match the graph's ranks and fixed
    /// dimensions. Returns every output, in graph output order, as owned `f32` tensors.
    pub fn run(&self, inputs: &[TensorF32]) -> Result<Vec<TensorF32>, ModelError> {
        if inputs.len() != self.inputs.len() {
            return Err(ModelError::InvalidInput(format!(
                "model `{}` takes {} inputs, got {}",
                self.id(),
                self.inputs.len(),
                inputs.len()
            )));
        }
        for (t, info) in inputs.iter().zip(&self.inputs) {
            let fits = t.shape().len() == info.shape.len()
                && t.shape()
                    .iter()
                    .zip(&info.shape)
                    .all(|(a, b)| *b < 0 || *a as i64 == *b);
            if !fits {
                return Err(ModelError::InvalidInput(format!(
                    "model `{}` input `{}` has shape {:?}, got {:?}",
                    self.id(),
                    info.name,
                    info.shape,
                    t.shape()
                )));
            }
        }
        let rt = |reason: String| ModelError::Runtime {
            id: self.entry.id.clone(),
            reason,
        };
        let mut values = Vec::with_capacity(inputs.len());
        for (t, info) in inputs.iter().zip(&self.inputs) {
            let view = TensorRef::from_array_view((t.shape().to_vec(), t.data()))
                .map_err(|e| rt(e.to_string()))?;
            values.push((info.name.clone(), view));
        }
        let mut session = self.acquire();
        let outputs = session.run(values).map_err(|e| rt(e.to_string()))?;
        let mut result = Vec::with_capacity(self.outputs.len());
        for info in &self.outputs {
            let value = outputs
                .get(info.name.as_str())
                .ok_or_else(|| rt(format!("output `{}` missing", info.name)))?;
            let (shape, data) = value
                .try_extract_tensor::<f32>()
                .map_err(|e| rt(e.to_string()))?;
            let shape = shape.iter().map(|d| (*d).max(0) as usize).collect();
            result.push(TensorF32::new(shape, data.to_vec())?);
        }
        Ok(result)
    }

    /// Run every session in the pool once on zero inputs (dynamic dimensions set to 1). Returns the
    /// slowest run.
    pub fn warm_up(&self) -> Result<Duration, ModelError> {
        let inputs: Vec<TensorF32> = self
            .inputs
            .iter()
            .map(|i| TensorF32::zeros(i.shape.iter().map(|d| (*d).max(1) as usize).collect()))
            .collect();
        let mut slowest = Duration::ZERO;
        for _ in 0..self.pool.len() {
            let t = Instant::now();
            self.run(&inputs)?;
            slowest = slowest.max(t.elapsed());
        }
        Ok(slowest)
    }
}

/// Verified models and assets with their ORT sessions. Create once at start-up and share
/// (`Runtime` is `Send + Sync`).
#[derive(Debug)]
pub struct Runtime {
    manifest: Manifest,
    models_dir: PathBuf,
    sessions: BTreeMap<String, Arc<ModelSession>>,
    assets: BTreeMap<String, Arc<[u8]>>,
}

impl Runtime {
    /// Read `<models_dir>/manifest.json`, verify the size and SHA-256 of every model (or of
    /// `options.models`) and asset, refuse `rejected` models, warn on `pending` ones, and create
    /// the sessions from the verified bytes. Requires [`init_onnxruntime`] first.
    pub fn load(models_dir: &Path, options: &RuntimeOptions) -> Result<Runtime, ModelError> {
        if onnxruntime_info().is_none() {
            return Err(ModelError::OrtNotInitialised);
        }
        let manifest = Manifest::load(models_dir)?;
        let verified = manifest.verify(models_dir, options.models.as_deref())?;
        let mut sessions = BTreeMap::new();
        for (id, bytes) in verified.models {
            let entry = manifest
                .model(&id)
                .cloned()
                .ok_or_else(|| ModelError::Unknown(id.clone()))?;
            let session = ModelSession::create(entry, &bytes, options)?;
            sessions.insert(id, Arc::new(session));
        }
        Ok(Runtime {
            manifest,
            models_dir: models_dir.to_path_buf(),
            sessions,
            assets: verified.assets,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// The session of model `model_id`.
    pub fn session(&self, model_id: &str) -> Result<Arc<ModelSession>, ModelError> {
        self.sessions
            .get(model_id)
            .cloned()
            .ok_or_else(|| ModelError::Unknown(model_id.to_owned()))
    }

    /// Loaded sessions, by id.
    pub fn sessions(&self) -> impl Iterator<Item = &Arc<ModelSession>> {
        self.sessions.values()
    }

    /// Loaded sessions with this role (and variant, when given).
    pub fn sessions_with_role(
        &self,
        role: ModelRole,
        variant: Option<&str>,
    ) -> Vec<Arc<ModelSession>> {
        self.sessions
            .values()
            .filter(|s| {
                s.role() == role && variant.is_none_or(|v| s.entry.variant.as_deref() == Some(v))
            })
            .cloned()
            .collect()
    }

    /// Verified bytes of asset `asset_id`.
    pub fn asset(&self, asset_id: &str) -> Result<Arc<[u8]>, ModelError> {
        self.assets
            .get(asset_id)
            .cloned()
            .ok_or_else(|| ModelError::Unknown(asset_id.to_owned()))
    }

    /// One inference per session (see [`ModelSession::warm_up`]). Returns the time per model.
    pub fn warm_up(&self) -> Result<Vec<(String, Duration)>, ModelError> {
        self.sessions
            .values()
            .map(|s| Ok((s.id().to_owned(), s.warm_up()?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_release_version_only_when_the_commit_matches() {
        let info = "ORT Build Info: git-branch=HEAD, git-commit-id=33ca962823, build type=Release";
        assert_eq!(build_info_commit(info), Some("33ca962823"));
        assert_eq!(build_info_commit("no commit"), None);
        let dir = std::env::temp_dir().join(format!("face-models-ortver-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = dir.join("onnxruntime.dll");
        std::fs::write(
            dir.join("VERSION_NUMBER"),
            "1.28.2
",
        )
        .unwrap();
        std::fs::write(
            dir.join("GIT_COMMIT_ID"),
            "33ca9628233dc8f002435e868d4c2e9f82766ca1
",
        )
        .unwrap();
        assert_eq!(release_version(&lib, info).as_deref(), Some("1.28.2"));
        let other = "ORT Build Info: git-branch=HEAD, git-commit-id=0123456789, build type=Release";
        assert_eq!(release_version(&lib, other), None);
        std::fs::remove_file(dir.join("GIT_COMMIT_ID")).unwrap();
        assert_eq!(release_version(&lib, other).as_deref(), Some("1.28.2"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn per_model_thread_overrides() {
        let mut o = RuntimeOptions::default();
        assert_eq!(o.intra_op_threads_for("auraface-v1"), 1);
        o.intra_op_threads_per_model.insert("auraface-v1".into(), 2);
        assert_eq!(o.intra_op_threads_for("auraface-v1"), 2);
        assert_eq!(o.intra_op_threads_for("mp-selfie-multiclass"), 1);
    }

    #[test]
    fn bundled_path_points_into_third_party() {
        let p = bundled_onnxruntime_path(Path::new("third_party"));
        assert!(p.starts_with(Path::new("third_party").join("onnxruntime").join("1.28.2")));
    }
}
