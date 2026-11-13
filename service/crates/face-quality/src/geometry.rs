//! Face frame, head box (crown to chin, ear to ear) and the API face geometry.

use face_imaging::geom::{Point, Rect};
use face_imaging::transform::RotatedRect;
use face_models::{Detection, Point3, SegmentClass, Segmentation, mesh};

use crate::pose::Pose;

/// Crown height above the eye line, in eye-to-chin distances, for a bare skull (anthropometric:
/// the eyes sit about halfway between chin and vertex).
pub const SKULL_CROWN_RATIO: f32 = 1.0;
/// Upper limit for the crown found by segmentation (hair), in eye-to-chin distances.
pub const MAX_HAIR_CROWN_RATIO: f32 = 1.25;
/// Ear-to-ear width as a multiple of the face contour width at ear level (landmarks 234-454).
pub const EAR_TO_EAR_FACTOR: f32 = 1.15;

/// A frame aligned with the eyes: origin at the iris midpoint (468/473), `u` along the eye line
/// from the image-left eye to the image-right eye, `v` perpendicular, down the face. Pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceFrame {
    pub origin: Point,
    pub u: Point,
    pub v: Point,
    /// Inter-eye (iris) distance, pixels.
    pub iod: f32,
}

impl FaceFrame {
    /// From 478 landmarks; `None` when the iris centres coincide.
    pub fn from_landmarks(lm: &[Point3]) -> Option<FaceFrame> {
        let (r, l) = (
            lm.get(mesh::RIGHT_IRIS_CENTER)?.xy(),
            lm.get(mesh::LEFT_IRIS_CENTER)?.xy(),
        );
        let d = l - r;
        let iod = d.norm();
        if !(iod.is_finite() && iod > 1e-3) {
            return None;
        }
        let u = d * (1.0 / iod);
        Some(FaceFrame {
            origin: r.midpoint(l),
            u,
            v: Point::new(-u.y, u.x),
            iod,
        })
    }

    /// Frame coordinates `(u, v)` of an image point.
    pub fn local(&self, p: Point) -> Point {
        let d = p - self.origin;
        Point::new(
            d.x * self.u.x + d.y * self.u.y,
            d.x * self.v.x + d.y * self.v.y,
        )
    }

    /// Image point of frame coordinates `(u, v)`.
    pub fn image(&self, local: Point) -> Point {
        self.origin + self.u * local.x + self.v * local.y
    }

    /// Rotation of the eye line, radians, clockwise in the image.
    pub fn rotation(&self) -> f32 {
        self.u.y.atan2(self.u.x)
    }
}

/// How the crown (top of the head) was placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrownSource {
    /// [`SKULL_CROWN_RATIO`] above the eyes.
    Anthropometric,
    /// Top of the hair / face-skin region found by segmentation (capped by
    /// [`MAX_HAIR_CROWN_RATIO`]).
    Segmentation,
}

/// Head geometry in image coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadGeometry {
    pub frame: FaceFrame,
    /// Iris midpoint.
    pub eye_mid: Point,
    /// Chin (landmark 152).
    pub chin: Point,
    /// Estimated crown.
    pub crown: Point,
    /// Chin-to-crown distance, pixels.
    pub height: f32,
    /// Ear-to-ear width estimate, pixels.
    pub width: f32,
    /// The head rectangle (crown to chin, ear to ear), aligned with the eye line.
    pub head: RotatedRect,
    /// Axis-aligned bounding box of `head`.
    pub head_box: Rect,
    pub crown_source: CrownSource,
}

impl HeadGeometry {
    /// Smallest margin between the head box and the border of a `width` x `height` image,
    /// divided by the head height (negative when the head is cut off): `HEAD_IN_FRAME`.
    pub fn in_frame_ratio(&self, width: u32, height: u32) -> f32 {
        let b = &self.head_box;
        let margins = [
            b.x,
            b.y,
            width as f32 - b.right(),
            height as f32 - b.bottom(),
        ];
        margins.into_iter().fold(f32::MAX, f32::min) / self.height
    }
}

/// Head geometry from 478 landmarks, refined with segmentation when given.
pub fn head_geometry(lm: &[Point3], seg: Option<&Segmentation>) -> Option<HeadGeometry> {
    if lm.len() < 478 {
        return None;
    }
    let frame = FaceFrame::from_landmarks(lm)?;
    let chin = lm[mesh::CHIN].xy();
    let d_chin = frame.local(chin).y;
    if d_chin.is_nan() || d_chin <= 0.1 * frame.iod {
        return None;
    }
    let ul = frame.local(lm[mesh::RIGHT_FACE_EDGE].xy()).x;
    let ur = frame.local(lm[mesh::LEFT_FACE_EDGE].xy()).x;
    let width = (ur - ul).abs() * EAR_TO_EAR_FACTOR;
    let uc = 0.5 * (ul + ur);

    let skull = SKULL_CROWN_RATIO * d_chin;
    let (crown_dist, crown_source) = match seg.and_then(|s| segmented_top(lm, &frame, s, d_chin)) {
        Some(top) if top > skull => (
            top.min(MAX_HAIR_CROWN_RATIO * d_chin),
            CrownSource::Segmentation,
        ),
        _ => (skull, CrownSource::Anthropometric),
    };
    let height = d_chin + crown_dist;
    let head = RotatedRect {
        center: frame.image(Point::new(uc, 0.5 * (d_chin - crown_dist))),
        width,
        height,
        rotation: frame.rotation(),
    };
    Some(HeadGeometry {
        frame,
        eye_mid: frame.origin,
        chin,
        crown: frame.image(Point::new(frame.local(chin).x, -crown_dist)),
        height,
        width,
        head,
        head_box: head.bounding_box(),
        crown_source,
    })
}

/// Distance above the eye line of the top of the head region (hair or face skin) on a band along
/// the face's vertical axis, scanning up from the forehead. `None` when the band starts outside
/// the segmentation.
fn segmented_top(lm: &[Point3], frame: &FaceFrame, seg: &Segmentation, d_chin: f32) -> Option<f32> {
    let forehead = frame.local(lm[mesh::FOREHEAD_TOP].xy());
    let midline = 0.5 * (forehead.x + frame.local(lm[mesh::CHIN].xy()).x);
    let half_band = 0.25 * frame.iod;
    let step = (0.02 * frame.iod).max(1.0);
    let limit = -1.6 * d_chin;
    let mut v = forehead.y;
    let mut top = None;
    while v >= limit {
        let (mut head, mut known) = (0u32, 0u32);
        for k in 0..9 {
            let u = midline - half_band + half_band * 2.0 * k as f32 / 8.0;
            if let Some(c) = seg.class_at(frame.image(Point::new(u, v))) {
                known += 1;
                if matches!(c, SegmentClass::Hair | SegmentClass::FaceSkin) {
                    head += 1;
                }
            }
        }
        if known == 0 {
            // Left the segmented area (image border): the head continues beyond it.
            return top.map(|_| -v);
        }
        if 2 * head < known {
            break;
        }
        top = Some(v);
        v -= step;
    }
    top.map(|t| -t)
}

/// What the API reports about the analysed face (`FaceInfo` in `contracts/openapi.yaml`), in the
/// coordinates of the analysed image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceGeometry {
    /// Bounding box of the 468 mesh landmarks (forehead to chin, cheek to cheek).
    pub bbox: Rect,
    /// ArcFace-order alignment points: landmarks 468, 473, 1, 61, 291.
    pub landmarks5: [Point; 5],
    /// Iris-centre distance, pixels.
    pub inter_eye_distance: f32,
    pub pose: Option<Pose>,
}

impl FaceGeometry {
    pub fn from_landmarks(lm: &[Point3], pose: Option<Pose>) -> Option<FaceGeometry> {
        if lm.len() < 478 {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &lm[..468] {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        let landmarks5 = mesh::ALIGNMENT_5.map(|i| lm[i].xy());
        Some(FaceGeometry {
            bbox: Rect::new(x0, y0, x1 - x0, y1 - y0),
            landmarks5,
            inter_eye_distance: landmarks5[0].distance(landmarks5[1]),
            pose,
        })
    }
}

/// Where a face is, as a detector reports it: box and the two eye keypoints (subject's right eye,
/// i.e. image-left, first). The document pipeline passes this to
/// [`crate::Analyzer::analyze_portrait`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceHint {
    pub bbox: Rect,
    pub right_eye: Point,
    pub left_eye: Point,
}

impl FaceHint {
    pub fn inter_eye_distance(&self) -> f32 {
        self.right_eye.distance(self.left_eye)
    }
}

impl From<&Detection> for FaceHint {
    fn from(d: &Detection) -> FaceHint {
        FaceHint {
            bbox: d.bbox,
            right_eye: d.right_eye(),
            left_eye: d.left_eye(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use face_imaging::transform::PixelRect;

    /// A simple synthetic face: eyes at (90,100) and (110,100) (IOD 20), chin 40 px below.
    pub(crate) fn synthetic_landmarks() -> Vec<Point3> {
        let mut lm = vec![Point3::new(100.0, 110.0, 0.0); 478];
        lm[mesh::RIGHT_IRIS_CENTER] = Point3::new(90.0, 100.0, 0.0);
        lm[mesh::LEFT_IRIS_CENTER] = Point3::new(110.0, 100.0, 0.0);
        lm[mesh::CHIN] = Point3::new(100.0, 140.0, 0.0);
        lm[mesh::FOREHEAD_TOP] = Point3::new(100.0, 82.0, 0.0);
        lm[mesh::RIGHT_FACE_EDGE] = Point3::new(76.0, 105.0, 0.0);
        lm[mesh::LEFT_FACE_EDGE] = Point3::new(124.0, 105.0, 0.0);
        lm
    }

    #[test]
    fn face_frame_round_trips_and_measures_roll() {
        let mut lm = synthetic_landmarks();
        let f = FaceFrame::from_landmarks(&lm).unwrap();
        assert_eq!(f.iod, 20.0);
        let p = Point::new(123.0, 77.0);
        assert!((f.image(f.local(p)) - p).norm() < 1e-4);
        assert!(f.rotation().abs() < 1e-6);
        // Image-right eye lower: rotated clockwise.
        lm[mesh::LEFT_IRIS_CENTER] = Point3::new(110.0, 110.0, 0.0);
        assert!(FaceFrame::from_landmarks(&lm).unwrap().rotation() > 0.0);
        lm[mesh::LEFT_IRIS_CENTER] = lm[mesh::RIGHT_IRIS_CENTER];
        assert!(FaceFrame::from_landmarks(&lm).is_none());
    }

    #[test]
    fn head_box_uses_anthropometry_and_segmentation() {
        let lm = synthetic_landmarks();
        let h = head_geometry(&lm, None).unwrap();
        // Eye-to-chin 40: crown 40 above the eyes, head height 80, width 48 * 1.15.
        assert_eq!(h.crown_source, CrownSource::Anthropometric);
        assert!((h.height - 80.0).abs() < 1e-4);
        assert!((h.width - 55.2).abs() < 1e-3);
        assert!((h.crown - Point::new(100.0, 60.0)).norm() < 1e-3);
        assert!((h.head_box.y - 60.0).abs() < 1e-3 && (h.head_box.bottom() - 140.0).abs() < 1e-3);
        // In a 200x200 image the tightest margin is the top one (60 px) / 80.
        assert!((h.in_frame_ratio(200, 200) - 0.75).abs() < 1e-4);
        // A crop that cuts the head off gives a negative ratio.
        assert!(h.in_frame_ratio(200, 130) < 0.0);

        // Hair reaching 48 px above the eyes (crown at y = 52) raises the crown.
        let roi = PixelRect {
            x: 0,
            y: 0,
            width: 200,
            height: 200,
        };
        let classes: Vec<u8> = (0..200 * 200)
            .map(|i| {
                let y = i / 200;
                if (52..140).contains(&y) { 1 } else { 0 }
            })
            .collect();
        let seg = Segmentation::from_classes(roi, classes).unwrap();
        let h = head_geometry(&lm, Some(&seg)).unwrap();
        assert_eq!(h.crown_source, CrownSource::Segmentation);
        assert!((h.height - 88.0).abs() < 1.5, "{}", h.height);
        // Big hair is capped at 1.25 x eye-to-chin above the eyes.
        let classes = vec![1u8; 200 * 200];
        let seg = Segmentation::from_classes(roi, classes).unwrap();
        let h = head_geometry(&lm, Some(&seg)).unwrap();
        assert!((h.height - 90.0).abs() < 1e-3, "{}", h.height);
    }

    #[test]
    fn face_geometry_reports_the_pinned_points() {
        let lm = synthetic_landmarks();
        let g = FaceGeometry::from_landmarks(&lm, None).unwrap();
        assert_eq!(g.landmarks5[0], Point::new(90.0, 100.0));
        assert_eq!(g.landmarks5[1], Point::new(110.0, 100.0));
        assert_eq!(g.inter_eye_distance, 20.0);
        let d = Detection {
            score: 0.9,
            bbox: Rect::new(1.0, 2.0, 3.0, 4.0),
            keypoints: [
                Point::new(5.0, 6.0),
                Point::new(7.0, 8.0),
                Point::default(),
                Point::default(),
                Point::default(),
                Point::default(),
            ],
        };
        let hint = FaceHint::from(&d);
        assert_eq!(hint.right_eye, Point::new(5.0, 6.0));
        assert!((hint.inter_eye_distance() - 8.0f32.sqrt()).abs() < 1e-6);
    }
}
