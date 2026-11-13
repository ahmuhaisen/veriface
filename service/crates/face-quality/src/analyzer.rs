//! The selfie and portrait pipelines (docs/architecture.md sections 3.1 and 3.2 step 5).

use std::time::{Duration, Instant};

use face_core::{Catalogue, CheckId, Group, Measurements, OutputStyle, Subject};
use face_imaging::RgbImage;
use face_imaging::geom::Point;
use face_imaging::transform::PixelRect;
use face_models::{
    BlendshapeScores, Blendshapes, Detection, DetectorConfig, FaceDetector, FaceLandmarker,
    FaceLandmarks, Runtime, Segmentation, Segmenter, ids, landmark_roi, segmentation_roi,
};

use crate::error::QualityError;
use crate::geometry::{FaceFrame, FaceGeometry, FaceHint, HeadGeometry, head_geometry};
use crate::measure::{self, FaceContext, Measure, Work};
use crate::output::render_output;
use crate::pose::{CanonicalFace, PITCH_OFFSET, PitchOffset, PoseEstimator, PoseFit};

/// Side of the landmark fallback crop around a full-range detection, in box widths
/// (`testdata/golden/README.md`).
pub const FALLBACK_CROP_BOX_WIDTHS: f32 = 2.2;

/// Analyzer settings.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalyzerConfig {
    /// Detector thresholds (both detectors). MediaPipe defaults: score 0.5, NMS IoU 0.3.
    pub detector: DetectorConfig,
    /// Minimum landmark presence to trust the landmarks (MediaPipe default 0.5).
    pub min_presence: f32,
    /// Pitch correction (see [`PITCH_OFFSET`]).
    pub pitch_offset: PitchOffset,
}

impl Default for AnalyzerConfig {
    fn default() -> Self {
        AnalyzerConfig {
            detector: DetectorConfig::default(),
            min_presence: 0.5,
            pitch_offset: PITCH_OFFSET,
        }
    }
}

/// Per-request options.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalysisOptions {
    /// `profile.faceCounting.minInterEyeDistanceRatio` for `SINGLE_FACE` (selfie only).
    pub min_inter_eye_distance_ratio: f64,
    /// `COMPRESSION_QUALITY` from the decoder (`codec::Decoded::jpeg_quality`), selfie only.
    /// `None` makes the check undetermined.
    pub compression_quality: Option<f64>,
    /// Measure only these checks (e.g. the profile's enabled ones); `None`: every check of the
    /// subject. Stages no requested check needs are skipped (segmentation in particular).
    pub checks: Option<Vec<CheckId>>,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        AnalysisOptions {
            min_inter_eye_distance_ratio: 0.02,
            compression_quality: None,
            checks: None,
        }
    }
}

/// How the landmark ROI was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LandmarkSource {
    /// Short-range BlazeFace on the whole image (what MediaPipe's FaceLandmarker does).
    WholeImage,
    /// Short-range BlazeFace on a square crop around the face (small faces; the goldens'
    /// `mediapipe-full-range-crop`).
    Crop(PixelRect),
    /// The ROI of the face hint / full-range detection itself.
    Hint,
}

/// Result of analysing one image.
#[derive(Clone, Debug)]
pub struct Analysis {
    /// One entry (value or undetermined) per measured check.
    pub measurements: Measurements,
    /// Faces found by the detector (selfie: full-range count; portrait: 1 when a hint is given).
    pub faces_detected: usize,
    /// The analysed face's geometry (API `face`), when landmarks were found.
    pub face: Option<FaceGeometry>,
    /// Head geometry for the output crop.
    pub head: Option<HeadGeometry>,
    /// 478 landmarks in image coordinates.
    pub landmarks: Option<FaceLandmarks>,
    pub landmark_source: Option<LandmarkSource>,
    pub blendshapes: Option<BlendshapeScores>,
    pub pose: Option<PoseFit>,
    pub segmentation: Option<Segmentation>,
    /// Wall time per stage.
    pub timings: Vec<(&'static str, Duration)>,
}

impl Analysis {
    /// The output image for a profile's `output.style`, `width` and `height` (see
    /// [`crate::output`]). `None` without head geometry.
    pub fn output(
        &self,
        image: &RgbImage,
        style: OutputStyle,
        width: u32,
        height: u32,
    ) -> Option<RgbImage> {
        render_output(image, self.head.as_ref()?, style, width, height)
    }
}

/// The non-risk checks of `subject`, in catalogue order.
pub fn subject_checks(subject: Subject) -> Vec<CheckId> {
    Catalogue::embedded()
        .checks()
        .iter()
        .filter(|d| d.subjects.contains(&subject) && d.group != Group::Risk)
        .map(|d| d.id)
        .filter(|id| !matches!(id, CheckId::DocumentDetected | CheckId::PortraitFound))
        .collect()
}

/// Holds the models and runs the pipelines. `Send + Sync`: create once, share.
#[derive(Debug, Clone)]
pub struct Analyzer {
    full: FaceDetector,
    short: FaceDetector,
    landmarker: FaceLandmarker,
    blendshapes: Blendshapes,
    segmenter: Segmenter,
    pose: PoseEstimator,
    config: AnalyzerConfig,
}

struct Located {
    landmarks: FaceLandmarks,
    /// The frame the landmark model ran on (whole image or crop), in image coordinates.
    frame: PixelRect,
    source: LandmarkSource,
}

struct Request<'a> {
    wanted: Vec<CheckId>,
    options: &'a AnalysisOptions,
}

impl Request<'_> {
    fn wants(&self, id: CheckId) -> bool {
        self.wanted.contains(&id)
    }

    fn any(&self, ids: &[CheckId]) -> bool {
        ids.iter().any(|id| self.wants(*id))
    }
}

fn set(m: &mut Measurements, req: &Request<'_>, id: CheckId, value: Measure) {
    if !req.wants(id) {
        return;
    }
    match value {
        Ok(v) if v.is_finite() => m.set_value(id, v),
        Ok(_) => m.set_undetermined(id, "non-finite value"),
        Err(cause) => m.set_undetermined(id, cause),
    }
}

/// Checks that use segmentation (classes, face-skin mask, or the crown estimate).
const SEGMENTATION_CHECKS: [CheckId; 12] = [
    CheckId::EyesVisible,
    CheckId::FaceOcclusion,
    CheckId::HeadCovering,
    CheckId::Eyeglasses,
    CheckId::BackgroundUniformity,
    CheckId::HeadInFrame,
    CheckId::HeadHeightRatio,
    CheckId::LuminanceMean,
    CheckId::UnderExposure,
    CheckId::OverExposure,
    CheckId::DynamicRange,
    CheckId::IlluminationUniformity,
];

impl Analyzer {
    /// Build from a loaded runtime: the v1 model ids (`face_models::ids`) and the canonical face
    /// asset.
    pub fn new(runtime: &Runtime, config: AnalyzerConfig) -> Result<Analyzer, QualityError> {
        let canonical = CanonicalFace::from_json(&runtime.asset(ids::CANONICAL_FACE)?)?;
        Ok(Analyzer {
            full: FaceDetector::new(
                runtime.session(ids::DETECTOR_FULL)?,
                config.detector.clone(),
            )?,
            short: FaceDetector::new(
                runtime.session(ids::DETECTOR_SHORT)?,
                config.detector.clone(),
            )?,
            landmarker: FaceLandmarker::new(runtime.session(ids::LANDMARKER)?)?,
            blendshapes: Blendshapes::new(runtime.session(ids::BLENDSHAPES)?)?,
            segmenter: Segmenter::new(runtime.session(ids::SEGMENTER)?)?,
            pose: PoseEstimator::new(canonical).with_pitch_offset(config.pitch_offset),
            config,
        })
    }

    pub fn config(&self) -> &AnalyzerConfig {
        &self.config
    }

    /// The full-range detector (counting, document portraits).
    pub fn full_range_detector(&self) -> &FaceDetector {
        &self.full
    }

    /// The short-range detector (selfie-distance faces; MediaPipe FaceLandmarker's detector).
    pub fn short_range_detector(&self) -> &FaceDetector {
        &self.short
    }

    pub fn pose_estimator(&self) -> &PoseEstimator {
        &self.pose
    }

    /// Faces in `image` (full-range BlazeFace, score order). The document pipeline's face finder.
    pub fn detect_faces(&self, image: &RgbImage) -> Result<Vec<Detection>, QualityError> {
        Ok(self.full.detect(image)?)
    }

    /// Landmarks for the face at `hint`: short-range detection on the whole image covering the
    /// hint box (MediaPipe FaceLandmarker); else short-range on a square crop of side 2.2 box
    /// widths around it; else the hint's own ROI. The first result with presence >=
    /// `min_presence` wins; otherwise the last attempt is returned.
    pub fn locate_landmarks(
        &self,
        image: &RgbImage,
        hint: &FaceHint,
    ) -> Result<Option<(FaceLandmarks, PixelRect, LandmarkSource)>, QualityError> {
        Ok(self
            .locate(image, hint)?
            .map(|l| (l.landmarks, l.frame, l.source)))
    }

    fn locate(&self, image: &RgbImage, hint: &FaceHint) -> Result<Option<Located>, QualityError> {
        let (w, h) = image.dimensions();
        let whole = PixelRect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        let covers = |d: &Detection, dx: f32, dy: f32| {
            let nose = d.keypoints[face_models::keypoint::NOSE_TIP];
            hint.bbox.contains(Point::new(nose.x + dx, nose.y + dy))
        };
        let best = |dets: Vec<Detection>, dx: f32, dy: f32| {
            dets.into_iter()
                .filter(|d| covers(d, dx, dy))
                .max_by(|a, b| a.inter_eye_distance().total_cmp(&b.inter_eye_distance()))
        };
        let mut last: Option<Located> = None;

        // 1. Short-range on the whole image.
        if let Some(d) = best(self.short.detect(image)?, 0.0, 0.0) {
            let lm = self.landmarker.landmarks(image, &d.landmark_roi())?;
            let ok = lm.presence >= self.config.min_presence;
            let located = Located {
                landmarks: lm,
                frame: whole,
                source: LandmarkSource::WholeImage,
            };
            if ok {
                return Ok(Some(located));
            }
            last = Some(located);
        }
        // 2. Short-range on a crop around the face: a square of 2.2 box widths centred on the
        //    (float) full-range box, clamped (golden rule).
        let center = hint.bbox.center();
        if let Some(crop_rect) =
            PixelRect::square_clamped(center, FALLBACK_CROP_BOX_WIDTHS * hint.bbox.width, w, h)
            && let Some(crop) = crop_rect.crop(image)
        {
            let (dx, dy) = (crop_rect.x as f32, crop_rect.y as f32);
            if let Some(d) = best(self.short.detect(&crop)?, dx, dy) {
                let lm = self
                    .landmarker
                    .landmarks(&crop, &d.landmark_roi())?
                    .translated(dx, dy);
                let ok = lm.presence >= self.config.min_presence;
                let located = Located {
                    landmarks: lm,
                    frame: crop_rect,
                    source: LandmarkSource::Crop(crop_rect),
                };
                if ok {
                    return Ok(Some(located));
                }
                last = Some(located);
            }
        }
        // 3. The hint's own ROI on the whole image.
        let roi = landmark_roi(&hint.bbox, hint.right_eye, hint.left_eye);
        if roi.width > 0.0 && roi.height > 0.0 {
            let lm = self.landmarker.landmarks(image, &roi)?;
            let ok = lm.presence >= self.config.min_presence;
            let located = Located {
                landmarks: lm,
                frame: whole,
                source: LandmarkSource::Hint,
            };
            if ok || last.is_none() {
                return Ok(Some(located));
            }
        }
        Ok(last)
    }

    /// Selfie assessment (`/v1/quality`, verification probe): measurements for every
    /// non-risk check whose subjects include `selfie` (or for `options.checks`).
    pub fn analyze_selfie(
        &self,
        image: &RgbImage,
        options: &AnalysisOptions,
    ) -> Result<Analysis, QualityError> {
        let all = subject_checks(Subject::Selfie);
        let wanted = match &options.checks {
            Some(c) => all.into_iter().filter(|id| c.contains(id)).collect(),
            None => all,
        };
        let req = Request { wanted, options };
        self.run(image, None, &req)
    }

    /// Portrait assessment (document reference): measurements for every non-risk check whose
    /// subjects include `portrait` (`PORTRAIT_FOUND` and `DOCUMENT_DETECTED` belong to the
    /// document pipeline). `image` is the upright portrait (or a crop around it); `hint` is the
    /// face in its coordinates (e.g. from [`Analyzer::detect_faces`]); without a hint the
    /// largest full-range face is used.
    pub fn analyze_portrait(
        &self,
        image: &RgbImage,
        hint: Option<&FaceHint>,
        options: &AnalysisOptions,
    ) -> Result<Analysis, QualityError> {
        let all = subject_checks(Subject::Portrait);
        let wanted = match &options.checks {
            Some(c) => all.into_iter().filter(|id| c.contains(id)).collect(),
            None => all,
        };
        let req = Request { wanted, options };
        self.run(image, hint, &req)
    }

    fn run(
        &self,
        image: &RgbImage,
        hint: Option<&FaceHint>,
        req: &Request<'_>,
    ) -> Result<Analysis, QualityError> {
        let (w, h) = image.dimensions();
        if w == 0 || h == 0 {
            return Err(QualityError::InvalidInput("empty image".to_owned()));
        }
        let mut m = Measurements::new();
        let mut timings = Vec::new();
        let analysis =
            |m: Measurements, timings: Vec<(&'static str, Duration)>, faces: usize| Analysis {
                measurements: m,
                faces_detected: faces,
                face: None,
                head: None,
                landmarks: None,
                landmark_source: None,
                blendshapes: None,
                pose: None,
                segmentation: None,
                timings,
            };
        set(
            &mut m,
            req,
            CheckId::ImageResolution,
            Ok(f64::from(w.min(h))),
        );
        set(
            &mut m,
            req,
            CheckId::CompressionQuality,
            req.options
                .compression_quality
                .ok_or_else(|| "compression quality not provided by the decoder".to_owned()),
        );

        // Detection.
        let t = Instant::now();
        let (primary, faces) = match hint {
            Some(hint) => (Some(*hint), 1usize),
            None => {
                let dets = self.full.detect(image)?;
                let min_iod = (req.options.min_inter_eye_distance_ratio * f64::from(w)) as f32;
                let counted = dets
                    .iter()
                    .filter(|d| d.inter_eye_distance() >= min_iod)
                    .count();
                set(&mut m, req, CheckId::FacePresent, Ok(dets.len() as f64));
                set(&mut m, req, CheckId::SingleFace, Ok(counted as f64));
                let primary = dets
                    .iter()
                    .max_by(|a, b| a.inter_eye_distance().total_cmp(&b.inter_eye_distance()))
                    .map(FaceHint::from);
                (primary, dets.len())
            }
        };
        timings.push(("detect", t.elapsed()));
        let needs_landmarks = req.wanted.iter().any(|id| {
            !matches!(
                id,
                CheckId::ImageResolution
                    | CheckId::CompressionQuality
                    | CheckId::FacePresent
                    | CheckId::SingleFace
            )
        });
        let Some(primary) = primary else {
            fill_undetermined(&mut m, req, "no face detected");
            return Ok(analysis(m, timings, faces));
        };
        if !needs_landmarks {
            return Ok(analysis(m, timings, faces));
        }

        // Landmarks.
        let t = Instant::now();
        let located = self.locate(image, &primary)?;
        timings.push(("landmarks", t.elapsed()));
        let Some(located) = located else {
            fill_undetermined(&mut m, req, "no landmarks");
            return Ok(analysis(m, timings, faces));
        };
        let lm = located.landmarks;
        if lm.presence < self.config.min_presence {
            let cause = format!(
                "landmark presence {:.2} below {:.2}",
                lm.presence, self.config.min_presence
            );
            fill_undetermined(&mut m, req, &cause);
            let mut a = analysis(m, timings, faces);
            a.landmarks = Some(lm);
            a.landmark_source = Some(located.source);
            return Ok(a);
        }
        let frame_offset = (located.frame.x as f32, located.frame.y as f32);
        let frame_points = lm.translated(-frame_offset.0, -frame_offset.1).points;

        // Blendshapes (in the frame the landmark model saw) and pose (position-invariant).
        let t = Instant::now();
        let blend = self.blendshapes.predict(&frame_points)?;
        timings.push(("blendshapes", t.elapsed()));
        let t = Instant::now();
        let pose = self.pose.fit(&lm.points);
        timings.push(("pose", t.elapsed()));

        // Segmentation (binding ROI rule).
        let t = Instant::now();
        let seg = if req.any(&SEGMENTATION_CHECKS) {
            match segmentation_roi(
                lm.point(face_models::mesh::RIGHT_IRIS_CENTER),
                lm.point(face_models::mesh::LEFT_IRIS_CENTER),
                w,
                h,
            ) {
                Some(roi) => Some(self.segmenter.segment(image, &roi)?),
                None => None,
            }
        } else {
            None
        };
        timings.push(("segment", t.elapsed()));

        // Measurements.
        let t = Instant::now();
        let frame = FaceFrame::from_landmarks(&lm.points);
        let head = head_geometry(&lm.points, seg.as_ref());
        let pose_angles = pose.map(|p| p.pose);
        let face = FaceGeometry::from_landmarks(&lm.points, pose_angles);
        match (frame, head) {
            (Some(frame), Some(head)) => {
                let ctx = FaceContext {
                    image,
                    landmarks: &lm.points,
                    frame,
                    head,
                    blendshapes: Some(&blend),
                    pose: pose_angles,
                    segmentation: seg.as_ref(),
                };
                self.measure(&ctx, req, &mut m);
            }
            _ => fill_undetermined(&mut m, req, "degenerate landmarks"),
        }
        fill_undetermined(&mut m, req, "not measured");
        timings.push(("measure", t.elapsed()));

        Ok(Analysis {
            measurements: m,
            faces_detected: faces,
            face,
            head,
            landmarks: Some(lm),
            landmark_source: Some(located.source),
            blendshapes: Some(blend),
            pose,
            segmentation: seg,
            timings,
        })
    }

    fn measure(&self, ctx: &FaceContext<'_>, req: &Request<'_>, m: &mut Measurements) {
        let (w, h) = ctx.image.dimensions();
        set(
            m,
            req,
            CheckId::InterEyeDistance,
            Ok(f64::from(ctx.frame.iod)),
        );
        let g = measure::geometry_ratios(&ctx.head, w, h);
        set(m, req, CheckId::HeadInFrame, Ok(g.head_in_frame));
        set(
            m,
            req,
            CheckId::EyeMidpointHorizontal,
            Ok(g.eye_mid_horizontal),
        );
        set(m, req, CheckId::EyeMidpointVertical, Ok(g.eye_mid_vertical));
        set(m, req, CheckId::HeadWidthRatio, Ok(g.head_width));
        set(m, req, CheckId::HeadHeightRatio, Ok(g.head_height));
        let pose = ctx.pose.ok_or_else(|| "pose fit failed".to_owned());
        set(
            m,
            req,
            CheckId::HeadPoseYaw,
            pose.clone().map(|p| f64::from(p.yaw)),
        );
        set(
            m,
            req,
            CheckId::HeadPosePitch,
            pose.clone().map(|p| f64::from(p.pitch)),
        );
        set(
            m,
            req,
            CheckId::HeadPoseRoll,
            pose.map(|p| f64::from(p.roll)),
        );
        set(m, req, CheckId::EyesOpen, measure::eyes_open(ctx));
        set(m, req, CheckId::GazeFrontal, measure::gaze(ctx));
        set(m, req, CheckId::MouthClosed, measure::mouth_open(ctx));
        set(m, req, CheckId::ExpressionNeutral, measure::expression(ctx));
        set(
            m,
            req,
            CheckId::BackgroundUniformity,
            measure::background_uniformity(ctx),
        );

        let pixel_checks = [
            CheckId::EyesVisible,
            CheckId::FaceOcclusion,
            CheckId::HeadCovering,
            CheckId::Eyeglasses,
            CheckId::LuminanceMean,
            CheckId::UnderExposure,
            CheckId::OverExposure,
            CheckId::DynamicRange,
            CheckId::IlluminationUniformity,
            CheckId::Sharpness,
        ];
        if !req.any(&pixel_checks) {
            return;
        }
        let Some(work) = Work::new(ctx) else {
            for id in pixel_checks {
                set(m, req, id, Err("face region outside the image".to_owned()));
            }
            return;
        };
        set(
            m,
            req,
            CheckId::EyesVisible,
            measure::eyes_visible(ctx, &work),
        );
        set(
            m,
            req,
            CheckId::FaceOcclusion,
            measure::face_occlusion(ctx, &work),
        );
        set(
            m,
            req,
            CheckId::HeadCovering,
            measure::head_covering(ctx, &work),
        );
        set(m, req, CheckId::Eyeglasses, measure::eyeglasses(ctx, &work));
        set(m, req, CheckId::Sharpness, measure::sharpness(ctx, &work));
        let full = if work.scale < 1.0 {
            Work::full_resolution(ctx)
        } else {
            None
        };
        let ill = measure::illumination(ctx, full.as_ref().unwrap_or(&work));
        set(m, req, CheckId::LuminanceMean, ill.luminance_mean);
        set(m, req, CheckId::UnderExposure, ill.under_exposure);
        set(m, req, CheckId::OverExposure, ill.over_exposure);
        set(m, req, CheckId::DynamicRange, ill.dynamic_range);
        set(m, req, CheckId::IlluminationUniformity, ill.uniformity);
    }
}

/// Mark every wanted check without a measurement as undetermined with `cause`.
fn fill_undetermined(m: &mut Measurements, req: &Request<'_>, cause: &str) {
    for id in &req.wanted {
        if m.get(*id).is_none() {
            m.set_undetermined(*id, cause);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_check_lists_follow_the_catalogue() {
        let selfie = subject_checks(Subject::Selfie);
        assert_eq!(selfie.len(), 28);
        assert!(selfie.contains(&CheckId::BackgroundUniformity));
        assert!(!selfie.contains(&CheckId::ProbeScreenArtifacts));
        let portrait = subject_checks(Subject::Portrait);
        assert_eq!(portrait.len(), 14);
        assert!(!portrait.contains(&CheckId::PortraitFound));
        assert!(portrait.contains(&CheckId::Sharpness));
    }
}
