//! The real engine: face-models (ONNX Runtime sessions), face-quality (selfie and portrait
//! analysis), face-document (portrait extraction) and face-verify (embedding, risk, judgement).
//!
//! Coordinates in API `face` fields are those of the uploaded image: the selfie analysis as is,
//! the reference from `Portrait::face`. Verification itself pairs the portrait crop with its
//! crop-space geometry (`face_verify::Subject::reference`).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use face_core::{
    Catalogue, CheckId, CheckSettings, DOCUMENT_ASSESSMENT, Measurements, OutputFormat,
    OutputSettings, Profile, SELFIE_ASSESSMENT, evaluate,
};
use face_document::{
    BoxError, DetectedFace, DocumentPortrait, FaceAnalysis, OutputSize, PortraitAnalyzer,
    ProcessedAs,
};
use face_imaging::RgbImage;
use face_imaging::codec::{self, Decoded};
use face_models::{ModelError, ModelSession, Runtime, RuntimeOptions, ids};
use face_quality::{Analysis, AnalysisOptions, Analyzer, AnalyzerConfig, FaceHint, QualityError};
use face_verify::{AuraFaceEmbedder, Embedder, Embedding, Subject, VerifyError};
use tracing::{info, warn};

use crate::config::{Config, StartupError};
use crate::dto::{
    BoundingBox, CalibrationRef, DocumentInfo, DocumentPortraitBody, EngineInfo, FaceInfo,
    ModelInfo, OperatingPointRef, OutputImage, OutputMediaType, Pose, QualityBody,
    ReferenceAssessment, ResolvedReferenceKind, ReviewStatus, RiskSummary, Rotation,
    SubjectAssessment, VerificationBody, point,
};
use crate::engine::{
    DocumentPortraitRequest, Engine, EngineError, QualityRequest, ReferenceKind, Stages,
    VerificationRequest,
};

/// The production engine.
pub struct PipelineEngine {
    runtime: Arc<Runtime>,
    analyzer: Analyzer,
    embedder: AuraFaceEmbedder<Arc<ModelSession>>,
    /// Sessions in the embedder's pool (parallel embedding needs at least 2).
    embedder_sessions: usize,
    info: EngineInfo,
    /// Run independent stages of a verification concurrently (see [`PipelineEngine::with_parallel_stages`]).
    parallel: bool,
}

/// Where to look for the ONNX Runtime library when `paths.ort_library` is not set.
pub fn onnxruntime_candidates(config: &Config) -> Vec<PathBuf> {
    if let Some(path) = &config.paths.ort_library {
        return vec![path.clone()];
    }
    let library = if cfg!(windows) {
        "onnxruntime.dll".to_owned()
    } else {
        format!("libonnxruntime.so.{}", face_models::ONNXRUNTIME_VERSION)
    };
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        candidates.push(dir.join(&library));
        candidates.push(face_models::bundled_onnxruntime_path(
            &dir.join("third_party"),
        ));
    }
    candidates.push(config.base_dir.join(&library));
    candidates.push(face_models::bundled_onnxruntime_path(
        &config.base_dir.join("third_party"),
    ));
    candidates
}

impl PipelineEngine {
    /// Load ONNX Runtime, verify the model manifest and create the sessions (startup steps 2-4
    /// of docs/architecture.md section 5, without the warm-up).
    pub fn load(config: &Config) -> Result<PipelineEngine, StartupError> {
        let candidates = onnxruntime_candidates(config);
        let library = candidates.iter().find(|p| p.is_file()).ok_or_else(|| {
            StartupError::new(format!(
                "ONNX Runtime library not found (looked for {}); run scripts/fetch-onnxruntime.ps1 \
                 or set paths.ort_library",
                candidates
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        face_models::init_onnxruntime(library, config.engine.onnxruntime_sha256.as_deref())
            .map_err(|e| StartupError::new(e.to_string()))?;
        let sessions = config
            .engine
            .effective_sessions(config.server.effective_concurrency());
        let options = RuntimeOptions {
            intra_op_threads: config.engine.intra_op_threads,
            sessions_per_model: sessions,
            graph_optimizations: true,
            models: None,
            intra_op_threads_per_model: config.engine.intra_op_threads_per_model.clone(),
        };
        let started = Instant::now();
        let runtime = Runtime::load(&config.paths.models, &options)
            .map_err(|e| StartupError::new(format!("models: {e}")))?;
        let unknown: Vec<String> = options
            .intra_op_threads_per_model
            .keys()
            .filter(|id| runtime.manifest().model(id).is_none())
            .map(|id| {
                format!("engine.intra_op_threads_per_model: `{id}` is not a model in manifest.json")
            })
            .collect();
        StartupError::from_problems(unknown)?;
        info!(
            models = %config.paths.models.display(),
            sessions_per_model = sessions,
            intra_op_threads = options.intra_op_threads,
            intra_op_overrides = ?options.intra_op_threads_per_model,
            load_ms = started.elapsed().as_millis() as u64,
            "models verified and loaded"
        );
        Ok(PipelineEngine::new(Arc::new(runtime))
            .map_err(StartupError::new)?
            .with_parallel_stages(config.engine.parallel_stages))
    }

    /// Build on a loaded runtime (every v1 model and the canonical-face asset).
    pub fn new(runtime: Arc<Runtime>) -> Result<PipelineEngine, String> {
        let analyzer = Analyzer::new(&runtime, AnalyzerConfig::default())
            .map_err(|e| format!("face analysis: {e}"))?;
        let session = runtime
            .session(ids::EMBEDDER)
            .map_err(|e| format!("embedder: {e}"))?;
        let embedder_sessions = session.pool_size();
        let embedder =
            AuraFaceEmbedder::from_session(session).map_err(|e| format!("embedder: {e}"))?;
        let info = EngineInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            catalogue_version: Catalogue::embedded().version(),
            models: runtime
                .manifest()
                .models
                .iter()
                .map(|m| ModelInfo {
                    id: m.id.clone(),
                    role: m.role.as_str().to_owned(),
                    sha256: m.sha256.clone(),
                    review_status: match m.review.status {
                        face_models::ReviewStatus::Approved => ReviewStatus::Approved,
                        face_models::ReviewStatus::Pending => ReviewStatus::Pending,
                        face_models::ReviewStatus::Rejected => ReviewStatus::Rejected,
                    },
                })
                .collect(),
        };
        Ok(PipelineEngine {
            runtime,
            analyzer,
            embedder,
            embedder_sessions,
            info,
            parallel: true,
        })
    }

    /// Verification runs the probe analysis concurrently with the reference extraction, and
    /// embeds the two faces on two sessions at once (when the embedder has at least two). Same
    /// computations, in a different order: results are identical; a verification uses up to two
    /// cores. On by default (`engine.parallel_stages`).
    pub fn with_parallel_stages(mut self, parallel: bool) -> PipelineEngine {
        self.parallel = parallel;
        self
    }

    fn analyze_selfie(
        &self,
        decoded: &Decoded,
        profile: &Profile,
        stages: &mut Stages,
    ) -> Result<Analysis, EngineError> {
        let options = AnalysisOptions {
            min_inter_eye_distance_ratio: profile.face_counting.min_inter_eye_distance_ratio,
            compression_quality: decoded.jpeg_quality.map(f64::from),
            checks: Some(enabled_checks(&profile.selfie.checks)),
        };
        let analysis = self
            .analyzer
            .analyze_selfie(&decoded.image, &options)
            .map_err(quality_error)?;
        record_analysis(stages, &analysis.timings);
        Ok(analysis)
    }

    /// Portrait extraction plus `COMPRESSION_QUALITY` from the file header.
    fn extract(
        &self,
        decoded: &Decoded,
        profile: &Profile,
        kind: ReferenceKind,
        output: Option<OutputSize>,
        stages: &mut Stages,
    ) -> Result<(DocumentPortrait, Measurements), EngineError> {
        let adapter = PortraitAdapter {
            analyzer: &self.analyzer,
            options: AnalysisOptions {
                min_inter_eye_distance_ratio: profile.face_counting.min_inter_eye_distance_ratio,
                compression_quality: None,
                checks: Some(enabled_checks(&profile.document.checks)),
            },
            timings: RefCell::new(Vec::new()),
        };
        let kind = match kind {
            ReferenceKind::Auto => face_document::ReferenceKind::Auto,
            ReferenceKind::Document => face_document::ReferenceKind::Document,
            ReferenceKind::Portrait => face_document::ReferenceKind::Portrait,
        };
        let started = Instant::now();
        let extracted = face_document::extract_portrait(
            &decoded.image,
            kind,
            self.analyzer.full_range_detector(),
            &adapter,
            output,
        )
        .map_err(|e| match e {
            face_document::DocumentError::FaceFinder(b)
            | face_document::DocumentError::PortraitAnalyzer(b) => boxed_error(&b),
        })?;
        let analysis = adapter.timings.into_inner();
        let analysed: Duration = analysis.iter().map(|(_, d)| *d).sum();
        stages.record("document", started.elapsed().saturating_sub(analysed));
        record_analysis(stages, &analysis);

        let mut measurements = extracted.measurements.clone();
        match decoded.jpeg_quality {
            Some(q) => measurements.set_value(CheckId::CompressionQuality, f64::from(q)),
            None => measurements.set_undetermined(
                CheckId::CompressionQuality,
                "the file's compression could not be estimated",
            ),
        }
        Ok((extracted, measurements))
    }
}

impl Engine for PipelineEngine {
    fn info(&self) -> &EngineInfo {
        &self.info
    }

    fn warm_up(&self) -> Result<(), EngineError> {
        let times = self.runtime.warm_up().map_err(model_error)?;
        for (id, elapsed) in times {
            info!(model = %id, ms = elapsed.as_millis() as u64, "warm-up inference");
        }
        Ok(())
    }

    fn check_profile(&self, profile: &Profile) -> Result<(), String> {
        let model = &profile.verification.model;
        if model == self.embedder.model_id() {
            Ok(())
        } else {
            Err(format!(
                "profile `{}`: verification.model `{model}` is not the loaded embedder `{}`",
                profile.name,
                self.embedder.model_id()
            ))
        }
    }

    fn assess_quality(
        &self,
        request: &QualityRequest<'_>,
        stages: &mut Stages,
    ) -> Result<QualityBody, EngineError> {
        let profile = request.profile;
        let analysis = self.analyze_selfie(request.image, profile, stages)?;
        let assessment = stages.time("evaluate", || {
            evaluate(
                Catalogue::embedded(),
                SELFIE_ASSESSMENT,
                &profile.selfie.checks,
                &analysis.measurements,
            )
        });
        let output = if request.include_output {
            stages.time("output", || {
                let settings = &profile.selfie.output;
                let rendered = match (settings.style, settings.width, settings.height) {
                    (Some(style), Some(w), Some(h)) => {
                        analysis.output(&request.image.image, style, w, h)
                    }
                    _ => None,
                };
                encode_output(rendered, settings)
            })
        } else {
            None
        };
        Ok(QualityBody {
            subject: SubjectAssessment::new(assessment, analysis.face.as_ref().map(quality_face)),
            output,
        })
    }

    fn extract_document_portrait(
        &self,
        request: &DocumentPortraitRequest<'_>,
        stages: &mut Stages,
    ) -> Result<DocumentPortraitBody, EngineError> {
        let profile = request.profile;
        let settings = &profile.document.output;
        let size = match (request.include_output, settings.width, settings.height) {
            (true, Some(width), Some(height)) => Some(OutputSize { width, height }),
            _ => None,
        };
        let (extracted, measurements) = self.extract(
            request.document,
            profile,
            ReferenceKind::Document,
            size,
            stages,
        )?;
        let assessment = stages.time("evaluate", || {
            evaluate(
                Catalogue::embedded(),
                DOCUMENT_ASSESSMENT,
                &profile.document.checks,
                &measurements,
            )
        });
        let output = stages.time("output", || encode_output(extracted.output, settings));
        let portrait = extracted.portrait.as_ref();
        Ok(DocumentPortraitBody {
            subject: SubjectAssessment::new(
                assessment,
                portrait.and_then(|p| p.face.as_ref()).map(document_face),
            ),
            document: extracted
                .document
                .as_ref()
                .map_or_else(DocumentInfo::not_detected, document_info),
            portrait_source_box: portrait.map(|p| BoundingBox::from(p.source_box)),
            output,
        })
    }

    fn verify(
        &self,
        request: &VerificationRequest<'_>,
        stages: &mut Stages,
    ) -> Result<VerificationBody, EngineError> {
        let profile = request.profile;
        let extract_reference = |stages: &mut Stages| {
            self.extract(
                request.reference,
                profile,
                request.reference_kind,
                None,
                stages,
            )
        };
        // The probe side: selfie analysis, then the probe-only risk measurements on the same
        // image and geometry the verification uses.
        let analyze_probe = |stages: &mut Stages| -> Result<_, EngineError> {
            let analysis = self.analyze_selfie(request.probe, profile, stages)?;
            let face = analysis.face.as_ref().map(to_document_geometry);
            let risk = stages.time("risk", || {
                face_verify::probe_measurements(profile, &request.probe.image, face.as_ref())
            });
            Ok((analysis, face, risk))
        };
        let ((probe, probe_face, probe_risk), (extracted, reference_measurements)) =
            if self.parallel {
                // Independent: the probe analysis on a scoped thread, the reference here.
                let span = tracing::Span::current();
                std::thread::scope(|scope| {
                    let probe = scope.spawn(|| {
                        let _entered = span.enter();
                        let mut probe_stages = Stages::default();
                        let analysis = analyze_probe(&mut probe_stages);
                        (analysis, probe_stages)
                    });
                    let reference = extract_reference(stages);
                    let (analysis, probe_stages) = probe
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
                    merge(stages, &probe_stages);
                    Ok::<_, EngineError>((analysis?, reference?))
                })?
            } else {
                let probe = analyze_probe(stages)?;
                (probe, extract_reference(stages)?)
            };

        let probe_subject = Subject {
            image: &request.probe.image,
            measurements: &probe.measurements,
            face: probe_face.as_ref(),
        };
        let reference_subject = match &extracted.portrait {
            Some(portrait) => Subject::reference(portrait, &reference_measurements),
            None => Subject {
                image: &request.reference.image,
                measurements: &reference_measurements,
                face: None,
            },
        };
        let embedder = TimedEmbedder {
            inner: &self.embedder,
            parallel: self.parallel && self.embedder_sessions >= 2,
            elapsed: Cell::new(Duration::ZERO),
        };
        let started = Instant::now();
        let verification = face_verify::verify_with_probe_risk(
            Catalogue::embedded(),
            profile,
            Some(&request.operating_point.id),
            &probe_subject,
            &reference_subject,
            &embedder,
            Some(&probe_risk),
        )
        .map_err(verify_error)?;
        let embedding = embedder.elapsed.get();
        stages.record("embed", embedding);
        stages.record("evaluate", started.elapsed().saturating_sub(embedding));

        let judgement = verification.judgement;
        Ok(VerificationBody {
            outcome: judgement.outcome,
            decision: judgement.decision,
            score: verification.score.filter(|s| s.is_finite()),
            threshold: verification.operating_point.threshold,
            operating_point: OperatingPointRef {
                id: verification.operating_point.id,
                target_fmr: verification.operating_point.target_fmr,
            },
            calibration: CalibrationRef {
                id: verification.calibration.id,
                status: verification.calibration.status,
            },
            probe: SubjectAssessment::new(
                verification.probe,
                probe.face.as_ref().map(quality_face),
            ),
            reference: ReferenceAssessment {
                subject: SubjectAssessment::new(
                    verification.reference,
                    extracted
                        .portrait
                        .as_ref()
                        .and_then(|p| p.face.as_ref())
                        .map(document_face),
                ),
                kind: match extracted.kind {
                    ProcessedAs::Document => ResolvedReferenceKind::Document,
                    ProcessedAs::Portrait => ResolvedReferenceKind::Portrait,
                },
                document: extracted.document.as_ref().map(document_info),
            },
            risk: RiskSummary {
                flagged: judgement.risk_flagged,
                checks: verification.risk.checks,
            },
            reasons: judgement.reasons,
        })
    }
}

/// face-quality's portrait analysis behind face-document's seam, with the profile's checks.
struct PortraitAdapter<'a> {
    analyzer: &'a Analyzer,
    options: AnalysisOptions,
    timings: RefCell<Vec<(&'static str, Duration)>>,
}

impl PortraitAnalyzer for PortraitAdapter<'_> {
    fn analyze_portrait(
        &self,
        crop: &RgbImage,
        face: &DetectedFace,
    ) -> Result<FaceAnalysis, BoxError> {
        let hint = FaceHint {
            bbox: face.bbox,
            right_eye: face.eyes[0],
            left_eye: face.eyes[1],
        };
        let analysis = self
            .analyzer
            .analyze_portrait(crop, Some(&hint), &self.options)?;
        self.timings
            .borrow_mut()
            .extend(analysis.timings.iter().copied());
        Ok(FaceAnalysis {
            face: analysis.face.as_ref().map(to_document_geometry),
            measurements: analysis.measurements,
        })
    }
}

/// Records the embedding time (the rest of `face_verify::verify` is evaluation). With
/// `parallel`, a batch is embedded one face per session, concurrently.
struct TimedEmbedder<'a> {
    inner: &'a AuraFaceEmbedder<Arc<ModelSession>>,
    parallel: bool,
    elapsed: Cell<Duration>,
}

impl Embedder for TimedEmbedder<'_> {
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn embed_batch(&self, aligned: &[&RgbImage]) -> Result<Vec<Embedding>, BoxError> {
        let started = Instant::now();
        let result = if self.parallel && aligned.len() > 1 {
            let span = tracing::Span::current();
            let inner = self.inner;
            std::thread::scope(|scope| {
                let others: Vec<_> = aligned[1..]
                    .iter()
                    .map(|face| {
                        let span = &span;
                        scope.spawn(move || {
                            let _entered = span.enter();
                            inner.embed(face)
                        })
                    })
                    .collect();
                let mut embeddings = vec![self.inner.embed(aligned[0])?];
                for other in others {
                    embeddings.push(
                        other
                            .join()
                            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))?,
                    );
                }
                Ok(embeddings)
            })
        } else {
            self.inner.embed_batch(aligned)
        };
        self.elapsed.set(self.elapsed.get() + started.elapsed());
        result
    }
}

// ----------------------------------------------------------------------------------- helpers

fn merge(into: &mut Stages, from: &Stages) {
    for (stage, elapsed) in from.entries() {
        into.record(stage, *elapsed);
    }
}

fn enabled_checks(settings: &CheckSettings) -> Vec<CheckId> {
    settings
        .iter()
        .filter(|(_, s)| s.enabled)
        .map(|(id, _)| *id)
        .collect()
}

/// Map face-quality stage names onto the service's stage set.
fn record_analysis(stages: &mut Stages, timings: &[(&'static str, Duration)]) {
    for (stage, elapsed) in timings {
        let stage = match *stage {
            "detect" => "detect",
            "landmarks" | "blendshapes" => "landmarks",
            "segment" => "segment",
            _ => "measure",
        };
        stages.record(stage, *elapsed);
    }
}

fn encode_output(image: Option<RgbImage>, settings: &OutputSettings) -> Option<OutputImage> {
    let image = image?;
    let (width, height) = image.dimensions();
    let encoded = match settings.format? {
        OutputFormat::Jpeg => codec::encode_jpeg_within(
            &image,
            usize::try_from(settings.max_bytes.unwrap_or(u64::MAX)).unwrap_or(usize::MAX),
            settings.min_jpeg_quality.unwrap_or(60),
        )
        .map(|e| (OutputMediaType::Jpeg, e.bytes)),
        OutputFormat::Png => codec::encode_png(&image).map(|b| (OutputMediaType::Png, b)),
    };
    match encoded {
        Ok((media_type, data)) => Some(OutputImage {
            media_type,
            width,
            height,
            data,
        }),
        Err(e) => {
            warn!(error = %e, "output image not produced");
            None
        }
    }
}

fn to_document_geometry(f: &face_quality::FaceGeometry) -> face_document::FaceGeometry {
    face_document::FaceGeometry {
        bbox: f.bbox,
        landmarks5: f.landmarks5,
        inter_eye_distance: f.inter_eye_distance,
        pose: f.pose.map(|p| face_document::Pose {
            yaw: p.yaw,
            pitch: p.pitch,
            roll: p.roll,
        }),
    }
}

fn quality_face(f: &face_quality::FaceGeometry) -> FaceInfo {
    document_face(&to_document_geometry(f))
}

fn document_face(f: &face_document::FaceGeometry) -> FaceInfo {
    FaceInfo {
        bounding_box: BoundingBox::from(f.bbox),
        landmarks5: f.landmarks5.map(point),
        inter_eye_distance_px: f64::from(f.inter_eye_distance),
        pose: f.pose.map(|p| Pose {
            yaw: f64::from(p.yaw),
            pitch: f64::from(p.pitch),
            roll: f64::from(p.roll),
        }),
    }
}

fn document_info(d: &face_document::DocumentInfo) -> DocumentInfo {
    DocumentInfo {
        detected: d.detected,
        confidence: d.confidence,
        corners: d.corners.map(|q| q.0.map(point)),
        orientation_degrees: match d.orientation_degrees {
            face_document::Rotation::R0 => Rotation::Deg0,
            face_document::Rotation::R90 => Rotation::Deg90,
            face_document::Rotation::R180 => Rotation::Deg180,
            face_document::Rotation::R270 => Rotation::Deg270,
        },
    }
}

// ------------------------------------------------------------------------------------ errors

/// A model that cannot load or run makes the engine unavailable (503); other failures are
/// internal (500).
fn is_model_failure(e: &ModelError) -> bool {
    !matches!(e, ModelError::InvalidInput(_))
}

fn model_error(e: ModelError) -> EngineError {
    if is_model_failure(&e) {
        EngineError::Unavailable(e.to_string())
    } else {
        EngineError::Internal(e.to_string())
    }
}

fn quality_error(e: QualityError) -> EngineError {
    match e {
        QualityError::Model(m) => model_error(m),
        e @ QualityError::Asset { .. } => EngineError::Unavailable(e.to_string()),
        e @ QualityError::InvalidInput(_) => EngineError::Internal(e.to_string()),
    }
}

/// Classify a boxed seam error by the first model or quality error in its source chain.
fn boxed_error(e: &BoxError) -> EngineError {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(e.as_ref());
    while let Some(err) = current {
        if let Some(m) = err.downcast_ref::<ModelError>() {
            return if is_model_failure(m) {
                EngineError::Unavailable(e.to_string())
            } else {
                EngineError::Internal(e.to_string())
            };
        }
        if let Some(QualityError::Model(m)) = err.downcast_ref::<QualityError>() {
            return if is_model_failure(m) {
                EngineError::Unavailable(e.to_string())
            } else {
                EngineError::Internal(e.to_string())
            };
        }
        current = err.source();
    }
    EngineError::Internal(e.to_string())
}

fn verify_error(e: VerifyError) -> EngineError {
    match e {
        VerifyError::Embedder(b) => boxed_error(&b),
        other => EngineError::Internal(other.to_string()),
    }
}
