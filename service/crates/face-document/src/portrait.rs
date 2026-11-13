//! The document-portrait pipeline (docs/architecture.md section 3.2).

use std::borrow::Cow;

use face_core::{CheckId, Measurements};
use face_imaging::RgbImage;
use face_imaging::geom::{Point, Quad, Rect};
use serde::{Deserialize, Serialize, Serializer};

use crate::DocumentError;
use crate::face::{DetectedFace, FaceFinder, FaceGeometry, PortraitAnalyzer};
use crate::orientation::{self, Rotation};
use crate::rectify::{
    self, DETECTION_THRESHOLD, Projective, RectifiedFrame, WORKING_LONG_SIDE, Working,
    downscale_area, fit_long_side, fit_within, warp_projective,
};

/// Largest plausible inter-eye distance of a document's portrait, as a fraction of the document
/// outline's long side. ID-1 cards and TD3 pages measure about 0.03-0.11; a portrait photo or a
/// tight photo crop about 0.17 or more, so an outline with a larger face is not a document (the
/// threshold is near the geometric midpoint).
///
/// PROVISIONAL: derived from ID-1/TD3 geometry and the lab's synthetic cards; not yet validated
/// on real documents, in particular vertical (portrait-orientation) cards, which measure closer
/// to the threshold (about 0.09-0.11).
pub const MAX_DOCUMENT_IED_RATIO: f32 = 0.13;
/// Side of the square analysis crop around the portrait, in inter-eye distances: head, hair and
/// shoulders with a margin, for landmarks and segmentation.
pub const ANALYSIS_CROP_IED: f32 = 7.0;
/// Height of the `face-crop` output region, in inter-eye distances (a head is about 3.6 IED).
pub const FACE_CROP_HEIGHT_IED: f32 = 5.2;
/// Eye line of the `face-crop` output, as a fraction of its height from the top.
pub const FACE_CROP_EYE_LINE: f32 = 0.42;

/// How to interpret the reference image (`referenceKind` in the API).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReferenceKind {
    /// Try the document path; fall back to the portrait path when no document is detected or
    /// no portrait is found on it.
    #[default]
    Auto,
    /// Rectify when an outline is detected (otherwise use the image as is), then find the
    /// portrait. Used by `/v1/document-portraits`.
    Document,
    /// The image is already a portrait: skip rectification, still search orientations.
    Portrait,
}

/// How the reference was processed (`reference.kind` in the API).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessedAs {
    Document,
    Portrait,
}

/// `DocumentInfo` in `contracts/openapi.yaml`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentInfo {
    /// True when an outline was detected and the portrait was searched on the rectified document.
    pub detected: bool,
    /// The `DOCUMENT_DETECTED` value: the confidence (0-1) of the outline used as the document,
    /// 0 when no plausible outline was found.
    pub confidence: f64,
    /// Outline corners in input-image coordinates, in DOCUMENT order: the document's own top-left
    /// corner (as read upright) first, then top-right, bottom-right, bottom-left. `None` when no
    /// outline was detected.
    #[serde(serialize_with = "serialize_corners")]
    pub corners: Option<Quad>,
    /// Clockwise rotation that makes the document and its portrait upright, relative to the input
    /// image.
    pub orientation_degrees: Rotation,
}

fn serialize_corners<S: Serializer>(corners: &Option<Quad>, s: S) -> Result<S::Ok, S::Error> {
    match corners {
        None => s.serialize_none(),
        Some(q) => s.serialize_some(&q.0.map(|p| [p.x, p.y])),
    }
}

/// Maximum size of the `face-crop` output image (the profile's `output.width` / `height`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputSize {
    pub width: u32,
    pub height: u32,
}

/// The selected portrait.
#[derive(Clone, Debug)]
pub struct Portrait {
    /// Where the portrait sits in the input image (input coordinates, before rotation): the
    /// detection box mapped back through the rotation and the homography. `portraitSourceBox`.
    pub source_box: Rect,
    /// The portrait's geometry in INPUT-image coordinates: the API's reference `face`
    /// (`FaceInfo`). Box and five points are mapped through [`Portrait::crop_to_input`] (the box
    /// becomes the bounding box of its mapped corners); the inter-eye distance is measured
    /// between the mapped eye centres; the pose is as analysed on the upright portrait (the
    /// values of the `HEAD_POSE_*` checks). `None` without landmarks.
    pub face: Option<FaceGeometry>,
    /// Upright full-resolution crop around the portrait (never upscaled). The portrait analysis
    /// ran on it, and verification aligns the reference face from it.
    pub crop: RgbImage,
    /// The portrait's geometry in `crop` coordinates, for alignment and embedding (the image and
    /// geometry of the verification reference). `None` without landmarks.
    pub crop_face: Option<FaceGeometry>,
    /// The detection that selected the portrait, in `crop` coordinates.
    pub detection: DetectedFace,
    /// Maps `crop` coordinates to input-image coordinates.
    pub crop_to_input: Projective,
}

/// Result of [`extract_portrait`].
#[derive(Clone, Debug)]
pub struct DocumentPortrait {
    /// How the reference was processed.
    pub kind: ProcessedAs,
    /// `DocumentInfo` when processed as a document; `None` for [`ProcessedAs::Portrait`].
    pub document: Option<DocumentInfo>,
    /// `DOCUMENT_DETECTED`, `PORTRAIT_FOUND`, `IMAGE_RESOLUTION` and the portrait analysis's
    /// `portrait`-subject measurements. The caller adds `COMPRESSION_QUALITY` (file header).
    pub measurements: Measurements,
    /// The selected portrait, when one was found.
    pub portrait: Option<Portrait>,
    /// The `face-crop` output (never upscaled), when requested and a portrait was found. The
    /// caller encodes it.
    pub output: Option<RgbImage>,
}

/// Find, select and analyse the portrait on a reference image (docs/architecture.md 3.2).
///
/// - `kind`: see [`ReferenceKind`]. `/v1/document-portraits` uses [`ReferenceKind::Document`].
/// - `finder`: the face detector; `analyzer`: face-quality's portrait analysis.
/// - `output`: the `face-crop` output size, or `None` for no output image.
///
/// Errors only when a seam fails; "no document" and "no portrait" are measurements.
pub fn extract_portrait(
    image: &RgbImage,
    kind: ReferenceKind,
    finder: &dyn FaceFinder,
    analyzer: &dyn PortraitAnalyzer,
    output: Option<OutputSize>,
) -> Result<DocumentPortrait, DocumentError> {
    let working = Working::new(image);
    let run = |frame: &Frame<'_>| process_frame(image, frame, finder, analyzer, output);
    match kind {
        ReferenceKind::Portrait => {
            // Rectification not attempted: DOCUMENT_DETECTED is undetermined.
            let outcome = run(&Frame::identity(image, &working))?;
            Ok(assemble(image, ProcessedAs::Portrait, None, None, outcome))
        }
        ReferenceKind::Document => {
            let (info, outcome) = match search_documents(image, &working, &run, true)? {
                DocumentSearch::Found {
                    quad,
                    confidence,
                    outcome,
                } => (
                    document_info(confidence, Some(quad), outcome.rotation),
                    *outcome,
                ),
                DocumentSearch::NotFound => {
                    let outcome = run(&Frame::identity(image, &working))?;
                    (document_info(0.0, None, outcome.rotation), outcome)
                }
            };
            let detected = info.confidence;
            Ok(assemble(
                image,
                ProcessedAs::Document,
                Some(info),
                Some(detected),
                outcome,
            ))
        }
        ReferenceKind::Auto => {
            if let DocumentSearch::Found {
                quad,
                confidence,
                outcome,
            } = search_documents(image, &working, &run, false)?
            {
                let info = document_info(confidence, Some(quad), outcome.rotation);
                return Ok(assemble(
                    image,
                    ProcessedAs::Document,
                    Some(info),
                    Some(confidence),
                    *outcome,
                ));
            }
            // Portrait fallback: no plausible document outline, so DOCUMENT_DETECTED is 0.
            let outcome = run(&Frame::identity(image, &working))?;
            Ok(assemble(
                image,
                ProcessedAs::Portrait,
                None,
                Some(0.0),
                outcome,
            ))
        }
    }
}

/// Outline candidates above [`DETECTION_THRESHOLD`] tried in order of confidence.
const DOCUMENT_ATTEMPTS: usize = 3;

enum DocumentSearch {
    /// The outline used as the document (rectified), with the portrait search on it.
    Found {
        quad: Quad,
        confidence: f64,
        outcome: Box<FrameOutcome>,
    },
    /// No plausible outline: none reached [`DETECTION_THRESHOLD`], or those that did turned out
    /// not to be documents.
    NotFound,
}

/// Try the most confident outlines: the first whose portrait is plausible for a document wins.
/// An outline whose face is too large ([`MAX_DOCUMENT_IED_RATIO`]) is a portrait photo, not a
/// document. When `allow_faceless`, the best outline is kept even without any face on it (a
/// document whose portrait is missing), unless a later outline has a plausible portrait.
fn search_documents(
    image: &RgbImage,
    working: &Working,
    run: &dyn Fn(&Frame<'_>) -> Result<FrameOutcome, DocumentError>,
    allow_faceless: bool,
) -> Result<DocumentSearch, DocumentError> {
    let candidates = rectify::find_document_quads(working);
    let mut faceless = None;
    let attempts = candidates
        .iter()
        .filter(|c| c.confidence >= DETECTION_THRESHOLD)
        .take(DOCUMENT_ATTEMPTS);
    for (i, candidate) in attempts.enumerate() {
        let Some(frame) = rectify::rectified_frame(&candidate.quad) else {
            continue;
        };
        let outcome = run(&Frame::rectified(&frame, image, working))?;
        match outcome.face_ratio {
            Some(r) if r > MAX_DOCUMENT_IED_RATIO => {}
            Some(_) => {
                return Ok(DocumentSearch::Found {
                    quad: candidate.quad,
                    confidence: candidate.confidence,
                    outcome: Box::new(outcome),
                });
            }
            None if i == 0 => faceless = Some((candidate.quad, candidate.confidence, outcome)),
            None => {}
        }
    }
    Ok(match faceless {
        Some((quad, confidence, outcome)) if allow_faceless => DocumentSearch::Found {
            quad,
            confidence,
            outcome: Box::new(outcome),
        },
        _ => DocumentSearch::NotFound,
    })
}

/// `quad` is the outline in image-space order (the rectified frame's corners); `rotation` turns
/// the rectified frame upright.
fn document_info(confidence: f64, quad: Option<Quad>, rotation: Rotation) -> DocumentInfo {
    DocumentInfo {
        detected: quad.is_some(),
        confidence,
        corners: quad.map(|q| document_order(q, rotation)),
        orientation_degrees: rotation,
    }
}

/// Outline corners in document order. The outline runs clockwise from the rectified frame's
/// origin; turning the frame clockwise by `rotation` brings the corner `(4 - quarter turns) % 4`
/// places along to the upright top-left.
pub(crate) fn document_order(quad: Quad, rotation: Rotation) -> Quad {
    let mut corners = quad.0;
    corners.rotate_left((4 - usize::from(rotation.degrees() / 90)) % 4);
    Quad(corners)
}

/// `document_detected`: the `DOCUMENT_DETECTED` value, `None` when rectification was not
/// attempted.
fn assemble(
    image: &RgbImage,
    kind: ProcessedAs,
    document: Option<DocumentInfo>,
    document_detected: Option<f64>,
    outcome: FrameOutcome,
) -> DocumentPortrait {
    // The analyser's portrait measurements first; the document-level values below win.
    let mut measurements = outcome.measurements;
    measurements.set_value(
        CheckId::ImageResolution,
        f64::from(image.width().min(image.height())),
    );
    match document_detected {
        Some(v) => measurements.set_value(CheckId::DocumentDetected, v),
        None => measurements.set_undetermined(
            CheckId::DocumentDetected,
            "rectification not attempted (referenceKind=portrait)",
        ),
    }
    measurements.set_value(CheckId::PortraitFound, outcome.candidates as f64);
    DocumentPortrait {
        kind,
        document,
        measurements,
        portrait: outcome.portrait,
        output: outcome.output,
    }
}

/// The image the portrait is searched in: the input itself, or a rectified document.
struct Frame<'a> {
    /// Size in full-resolution pixels, before rotation.
    width: u32,
    height: u32,
    /// Frame coordinates to input coordinates.
    to_input: Projective,
    /// The frame at search resolution (long side at most [`WORKING_LONG_SIDE`]).
    search: Cow<'a, RgbImage>,
}

impl<'a> Frame<'a> {
    fn identity(image: &RgbImage, working: &'a Working) -> Frame<'a> {
        Frame {
            width: image.width(),
            height: image.height(),
            to_input: Projective::IDENTITY,
            search: Cow::Borrowed(&working.rgb),
        }
    }

    fn rectified(frame: &RectifiedFrame, image: &RgbImage, working: &Working) -> Frame<'static> {
        let (sw, sh) = fit_long_side(frame.width, frame.height, WORKING_LONG_SIDE);
        let search = if (sw, sh) == (frame.width, frame.height) {
            // Small enough: sample the full-resolution input directly.
            warp_projective(image, &frame.to_input, sw, sh)
        } else {
            // Downscaled search image: sample the area-averaged working copy (no aliasing).
            let to_working = Projective::scale(
                f64::from(frame.width) / f64::from(sw),
                f64::from(frame.height) / f64::from(sh),
            )
            .then(&frame.to_input)
            .then(&Projective::scale(working.sx, working.sy));
            warp_projective(&working.rgb, &to_working, sw, sh)
        };
        Frame {
            width: frame.width,
            height: frame.height,
            to_input: frame.to_input,
            search: Cow::Owned(search),
        }
    }

    fn search_to_frame(&self) -> Projective {
        Projective::scale(
            f64::from(self.width) / f64::from(self.search.width().max(1)),
            f64::from(self.height) / f64::from(self.search.height().max(1)),
        )
    }
}

struct FrameOutcome {
    rotation: Rotation,
    candidates: usize,
    /// Inter-eye distance of the selected face over the frame's long side, when a face was found.
    face_ratio: Option<f32>,
    portrait: Option<Portrait>,
    measurements: Measurements,
    output: Option<RgbImage>,
}

impl FrameOutcome {
    fn none(rotation: Rotation, candidates: usize) -> FrameOutcome {
        FrameOutcome {
            rotation,
            candidates,
            face_ratio: None,
            portrait: None,
            measurements: Measurements::new(),
            output: None,
        }
    }
}

fn process_frame(
    image: &RgbImage,
    frame: &Frame<'_>,
    finder: &dyn FaceFinder,
    analyzer: &dyn PortraitAnalyzer,
    output: Option<OutputSize>,
) -> Result<FrameOutcome, DocumentError> {
    let Some(found) = orientation::search(&frame.search, finder)? else {
        return Ok(FrameOutcome::none(Rotation::R0, 0));
    };
    let rotation = found.rotation;
    let mut outcome = FrameOutcome::none(rotation, found.faces.len());

    let (sw, sh) = frame.search.dimensions();
    let (fw, fh) = (f64::from(frame.width), f64::from(frame.height));
    let upright = rotation.transform(fw, fh);
    let Some(search_to_upright) = rotation
        .transform(f64::from(sw), f64::from(sh))
        .invert()
        .map(|unrotate| unrotate.then(&frame.search_to_frame()).then(&upright))
    else {
        return Ok(outcome);
    };
    let Some(upright_to_input) = upright.invert().map(|u| u.then(&frame.to_input)) else {
        return Ok(outcome);
    };
    let (uw, uh) = rotation.rotated_size(frame.width, frame.height);
    let Some(face) = map_face(&found.faces[found.selected], &search_to_upright) else {
        return Ok(outcome);
    };
    outcome.face_ratio = Some(face.inter_eye_distance() / uw.max(uh).max(1) as f32);

    let region = analysis_region(&face, uw, uh);
    let (rx, ry) = (f64::from(region.0), f64::from(region.1));
    let crop_to_input = Projective::translate(rx, ry).then(&upright_to_input);
    let crop = warp_projective(image, &crop_to_input, region.2, region.3);
    let Some(detection) = map_face(&face, &Projective::translate(-rx, -ry)) else {
        return Ok(outcome);
    };
    let analysis = analyzer
        .analyze_portrait(&crop, &detection)
        .map_err(DocumentError::PortraitAnalyzer)?;
    outcome.output =
        output.and_then(|size| face_crop(&crop, analysis.face.as_ref(), &detection, size));
    outcome.measurements = analysis.measurements;
    outcome.portrait = Some(Portrait {
        source_box: map_rect(&detection.bbox, &crop_to_input).unwrap_or_default(),
        face: analysis
            .face
            .as_ref()
            .and_then(|g| map_geometry(g, &crop_to_input)),
        crop,
        crop_face: analysis.face,
        detection,
        crop_to_input,
    });
    Ok(outcome)
}

/// Face geometry mapped to another image: five points mapped, box as the bounding box of its
/// mapped corners, inter-eye distance between the mapped eye centres, pose unchanged.
fn map_geometry(g: &FaceGeometry, t: &Projective) -> Option<FaceGeometry> {
    let mut landmarks5 = g.landmarks5;
    for p in &mut landmarks5 {
        *p = map_point(*p, t)?;
    }
    Some(FaceGeometry {
        bbox: map_rect(&g.bbox, t)?,
        inter_eye_distance: landmarks5[0].distance(landmarks5[1]),
        landmarks5,
        pose: g.pose,
    })
}

/// Integer region `(x, y, width, height)` of the upright frame around the face: a square of
/// [`ANALYSIS_CROP_IED`] inter-eye distances centred slightly below the eyes, clipped to the frame.
fn analysis_region(face: &DetectedFace, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let ied = face.inter_eye_distance();
    let side = (ANALYSIS_CROP_IED * ied)
        .max(2.5 * face.bbox.width.max(face.bbox.height))
        .max(32.0);
    let mid = face.eye_midpoint();
    let (cx, cy) = (mid.x, mid.y + 0.3 * ied);
    let clamp = |v: f32, max: u32| v.clamp(0.0, max as f32) as u32;
    let x0 = clamp((cx - 0.5 * side).floor(), width.saturating_sub(1));
    let y0 = clamp((cy - 0.5 * side).floor(), height.saturating_sub(1));
    let x1 = clamp((cx + 0.5 * side).ceil(), width).max(x0 + 1);
    let y1 = clamp((cy + 0.5 * side).ceil(), height).max(y0 + 1);
    (x0, y0, x1 - x0, y1 - y0)
}

fn map_point(p: Point, t: &Projective) -> Option<Point> {
    t.apply(p)
}

/// Axis-aligned bounding box of a rectangle's mapped corners.
fn map_rect(r: &Rect, t: &Projective) -> Option<Rect> {
    let corners = [
        Point::new(r.x, r.y),
        Point::new(r.right(), r.y),
        Point::new(r.right(), r.bottom()),
        Point::new(r.x, r.bottom()),
    ];
    let mapped: Vec<Point> = corners
        .iter()
        .map(|p| map_point(*p, t))
        .collect::<Option<_>>()?;
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &mapped {
        (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
    }
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

fn map_face(face: &DetectedFace, t: &Projective) -> Option<DetectedFace> {
    Some(DetectedFace {
        bbox: map_rect(&face.bbox, t)?,
        score: face.score,
        eyes: [map_point(face.eyes[0], t)?, map_point(face.eyes[1], t)?],
    })
}

/// The `face-crop` output: a region with the output's aspect ratio around the face (eyes at
/// [`FACE_CROP_EYE_LINE`], height [`FACE_CROP_HEIGHT_IED`] inter-eye distances), shifted and
/// shrunk to stay inside the crop (never padded), then downscaled to fit `size` (never upscaled).
fn face_crop(
    crop: &RgbImage,
    geometry: Option<&FaceGeometry>,
    detection: &DetectedFace,
    size: OutputSize,
) -> Option<RgbImage> {
    let (mid, ied) = match geometry {
        Some(g) if g.inter_eye_distance > 0.0 => (
            g.landmarks5[0].midpoint(g.landmarks5[1]),
            g.inter_eye_distance,
        ),
        _ => (detection.eye_midpoint(), detection.inter_eye_distance()),
    };
    if ied.is_nan() || ied <= 0.0 || size.width == 0 || size.height == 0 || crop.width() == 0 {
        return None;
    }
    let (cw, ch) = (crop.width() as f32, crop.height() as f32);
    let aspect = size.width as f32 / size.height as f32;
    let mut h = FACE_CROP_HEIGHT_IED * ied;
    let mut w = h * aspect;
    if w > cw {
        (w, h) = (cw, cw / aspect);
    }
    if h > ch {
        (w, h) = (ch * aspect, ch);
    }
    let x = (mid.x - 0.5 * w).clamp(0.0, (cw - w).max(0.0)).round() as u32;
    let y = (mid.y - FACE_CROP_EYE_LINE * h)
        .clamp(0.0, (ch - h).max(0.0))
        .round() as u32;
    let w = (w.round() as u32).clamp(1, crop.width() - x);
    let h = (h.round() as u32).clamp(1, crop.height() - y);
    let region = image::imageops::crop_imm(crop, x, y, w, h).to_image();
    let (ow, oh) = fit_within(w, h, size.width, size.height);
    Some(downscale_area(&region, ow, oh))
}
