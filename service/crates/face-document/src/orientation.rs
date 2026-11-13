//! Orientation search (four 90-degree rotations) and portrait selection.

use std::borrow::Cow;

use face_imaging::RgbImage;
use serde::{Serialize, Serializer};

use crate::DocumentError;
use crate::face::{DetectedFace, FaceFinder};
use crate::rectify::Projective;

/// A face counts as "large" when its inter-eye distance is at least this fraction of the largest
/// one found in any rotation. Only large faces compete in the orientation search.
pub const LARGE_FACE_RATIO: f32 = 0.5;

/// Clockwise rotation by a multiple of 90 degrees. Serialises as the number of degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

impl Rotation {
    pub const ALL: [Rotation; 4] = [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270];

    pub fn degrees(self) -> u16 {
        match self {
            Rotation::R0 => 0,
            Rotation::R90 => 90,
            Rotation::R180 => 180,
            Rotation::R270 => 270,
        }
    }

    /// The image rotated clockwise by this angle.
    pub fn rotate(self, image: &RgbImage) -> RgbImage {
        match self {
            Rotation::R0 => image.clone(),
            Rotation::R90 => image::imageops::rotate90(image),
            Rotation::R180 => image::imageops::rotate180(image),
            Rotation::R270 => image::imageops::rotate270(image),
        }
    }

    /// Size of a `width` x `height` image after this rotation.
    pub fn rotated_size(self, width: u32, height: u32) -> (u32, u32) {
        match self {
            Rotation::R0 | Rotation::R180 => (width, height),
            Rotation::R90 | Rotation::R270 => (height, width),
        }
    }

    /// Maps continuous coordinates of a `width` x `height` image into the rotated image
    /// (consistent with [`Rotation::rotate`]).
    pub fn transform(self, width: f64, height: f64) -> Projective {
        let m = match self {
            Rotation::R0 => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            Rotation::R90 => [[0.0, -1.0, height], [1.0, 0.0, 0.0]],
            Rotation::R180 => [[-1.0, 0.0, width], [0.0, -1.0, height]],
            Rotation::R270 => [[0.0, 1.0, 0.0], [-1.0, 0.0, width]],
        };
        Projective([m[0], m[1], [0.0, 0.0, 1.0]])
    }

    fn preference(self) -> u8 {
        // On ties prefer no rotation, then quarter turns, then upside down.
        match self {
            Rotation::R0 => 3,
            Rotation::R90 | Rotation::R270 => 2,
            Rotation::R180 => 1,
        }
    }
}

impl Serialize for Rotation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16(self.degrees())
    }
}

/// Result of the orientation search.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OrientationSearch {
    /// Clockwise rotation that makes the portrait upright.
    pub(crate) rotation: Rotation,
    /// All faces found in that rotation, in rotated-image coordinates.
    pub(crate) faces: Vec<DetectedFace>,
    /// Index in `faces` of the selected portrait (largest inter-eye distance).
    pub(crate) selected: usize,
}

/// Run the face finder on the four rotations of `image` and keep the rotation with the strongest
/// large face. `None` when no rotation has a face.
pub(crate) fn search(
    image: &RgbImage,
    finder: &dyn FaceFinder,
) -> Result<Option<OrientationSearch>, DocumentError> {
    let mut runs: Vec<(Rotation, Vec<DetectedFace>)> = Vec::with_capacity(4);
    for rotation in Rotation::ALL {
        let rotated = match rotation {
            Rotation::R0 => Cow::Borrowed(image),
            r => Cow::Owned(r.rotate(image)),
        };
        let faces = finder
            .find_faces(&rotated)
            .map_err(DocumentError::FaceFinder)?
            .into_iter()
            .filter(|f| f.score.is_finite() && f.inter_eye_distance().is_finite())
            .collect();
        runs.push((rotation, faces));
    }

    let largest = runs
        .iter()
        .flat_map(|(_, faces)| faces)
        .map(DetectedFace::inter_eye_distance)
        .fold(0.0f32, f32::max);
    if largest <= 0.0 {
        return Ok(None);
    }
    let large = LARGE_FACE_RATIO * largest;
    let best = runs
        .iter()
        .filter_map(|(rotation, faces)| {
            faces
                .iter()
                .filter(|f| f.inter_eye_distance() >= large)
                .map(|f| (*rotation, f.score, f.inter_eye_distance()))
                .max_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
        })
        .max_by(|a, b| {
            a.1.total_cmp(&b.1)
                .then(a.2.total_cmp(&b.2))
                .then(a.0.preference().cmp(&b.0.preference()))
        });
    let Some((rotation, _, _)) = best else {
        return Ok(None);
    };
    let faces = runs
        .into_iter()
        .find(|(r, _)| *r == rotation)
        .map(|(_, faces)| faces)
        .unwrap_or_default();
    let selected = faces
        .iter()
        .enumerate()
        .max_by(|a, b| {
            a.1.inter_eye_distance()
                .total_cmp(&b.1.inter_eye_distance())
                .then(a.1.score.total_cmp(&b.1.score))
        })
        .map_or(0, |(i, _)| i);
    Ok(Some(OrientationSearch {
        rotation,
        faces,
        selected,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::face::BoxError;
    use face_imaging::geom::{Point, Rect};
    use image::Rgb;
    use std::cell::RefCell;

    #[test]
    fn transforms_match_the_image_rotations() {
        let mut img = RgbImage::new(5, 3);
        img.put_pixel(1, 0, Rgb([255, 0, 0]));
        for r in Rotation::ALL {
            let rotated = r.rotate(&img);
            assert_eq!(rotated.dimensions(), r.rotated_size(5, 3));
            // Centre of pixel (1, 0) maps to the centre of the red pixel in the rotated image.
            let p = r.transform(5.0, 3.0).apply_f64(1.5, 0.5).unwrap();
            let (x, y) = ((p.0 - 0.5).round() as u32, (p.1 - 0.5).round() as u32);
            assert_eq!(rotated.get_pixel(x, y).0, [255, 0, 0], "{r:?}");
            assert_eq!(
                serde_json::to_value(r).unwrap(),
                serde_json::json!(r.degrees())
            );
        }
    }

    fn face(ied: f32, score: f32) -> DetectedFace {
        DetectedFace {
            bbox: Rect::new(10.0, 10.0, 2.0 * ied, 2.0 * ied),
            score,
            eyes: [Point::new(10.0, 20.0), Point::new(10.0 + ied, 20.0)],
        }
    }

    /// Returns scripted faces per call (one call per rotation, in `Rotation::ALL` order).
    struct Scripted(RefCell<Vec<Vec<DetectedFace>>>);

    impl FaceFinder for Scripted {
        fn find_faces(&self, _: &RgbImage) -> Result<Vec<DetectedFace>, BoxError> {
            Ok(self.0.borrow_mut().remove(0))
        }
    }

    #[test]
    fn keeps_the_rotation_with_the_strongest_large_face_and_selects_the_largest() {
        let img = RgbImage::new(8, 8);
        // 0: a confident but tiny face; 90: the real portrait plus a ghost; 270: weaker portrait.
        let finder = Scripted(RefCell::new(vec![
            vec![face(5.0, 0.99)],
            vec![face(16.0, 0.6), face(40.0, 0.93)],
            vec![],
            vec![face(38.0, 0.7)],
        ]));
        let s = search(&img, &finder).unwrap().unwrap();
        assert_eq!(s.rotation, Rotation::R90);
        assert_eq!(s.faces.len(), 2);
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn no_faces_gives_none_and_ties_prefer_no_rotation() {
        let img = RgbImage::new(8, 8);
        let none = Scripted(RefCell::new(vec![vec![], vec![], vec![], vec![]]));
        assert!(search(&img, &none).unwrap().is_none());
        let tie = Scripted(RefCell::new(vec![
            vec![face(30.0, 0.9)],
            vec![],
            vec![face(30.0, 0.9)],
            vec![],
        ]));
        assert_eq!(search(&img, &tie).unwrap().unwrap().rotation, Rotation::R0);
    }
}
