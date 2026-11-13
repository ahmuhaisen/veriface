//! MediaPipe face blendshapes: 52 ARKit-style expression coefficients from 146 landmarks.
//!
//! Input (`FaceBlendshapesGraph`): the landmarks listed in [`BLENDSHAPE_LANDMARKS`], in that
//! order, as `(x, y)` pixels of the image the landmarks were computed on, shape `[1, 146, 2]`.
//! The model normalises internally. Output: 52 coefficients in [0, 1].

use std::sync::Arc;

use crate::error::ModelError;
use crate::landmarks::Point3;
use crate::runtime::ModelSession;
use crate::tensor::TensorF32;

/// The landmark subset the blendshape model takes (`kLandmarksSubsetIdxs`), in input order.
pub const BLENDSHAPE_LANDMARKS: [usize; 146] = [
    0, 1, 4, 5, 6, 7, 8, 10, 13, 14, 17, 21, 33, 37, 39, 40, 46, 52, 53, 54, 55, 58, 61, 63, 65,
    66, 67, 70, 78, 80, 81, 82, 84, 87, 88, 91, 93, 95, 103, 105, 107, 109, 127, 132, 133, 136,
    144, 145, 146, 148, 149, 150, 152, 153, 154, 155, 157, 158, 159, 160, 161, 162, 163, 168, 172,
    173, 176, 178, 181, 185, 191, 195, 197, 234, 246, 249, 251, 263, 267, 269, 270, 276, 282, 283,
    284, 285, 288, 291, 293, 295, 296, 297, 300, 308, 310, 311, 312, 314, 317, 318, 321, 323, 324,
    332, 334, 336, 338, 356, 361, 362, 365, 373, 374, 375, 377, 378, 379, 380, 381, 382, 384, 385,
    386, 387, 388, 389, 390, 397, 398, 400, 402, 405, 409, 415, 454, 466, 468, 469, 470, 471, 472,
    473, 474, 475, 476, 477,
];

macro_rules! blendshapes {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        /// A blendshape coefficient, in model output order. "Left" / "Right" are the subject's
        /// (`EyeBlinkLeft` is the subject's left eye, image-right).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Blendshape { $($variant),+ }

        impl Blendshape {
            /// All 52, in model output order.
            pub const ALL: [Blendshape; 52] = [$(Blendshape::$variant),+];

            /// MediaPipe's name, e.g. `eyeBlinkLeft`.
            pub fn name(self) -> &'static str {
                match self { $(Blendshape::$variant => $name),+ }
            }

            /// Index in the model output.
            pub fn index(self) -> usize {
                self as usize
            }
        }
    };
}

blendshapes! {
    Neutral => "_neutral",
    BrowDownLeft => "browDownLeft",
    BrowDownRight => "browDownRight",
    BrowInnerUp => "browInnerUp",
    BrowOuterUpLeft => "browOuterUpLeft",
    BrowOuterUpRight => "browOuterUpRight",
    CheekPuff => "cheekPuff",
    CheekSquintLeft => "cheekSquintLeft",
    CheekSquintRight => "cheekSquintRight",
    EyeBlinkLeft => "eyeBlinkLeft",
    EyeBlinkRight => "eyeBlinkRight",
    EyeLookDownLeft => "eyeLookDownLeft",
    EyeLookDownRight => "eyeLookDownRight",
    EyeLookInLeft => "eyeLookInLeft",
    EyeLookInRight => "eyeLookInRight",
    EyeLookOutLeft => "eyeLookOutLeft",
    EyeLookOutRight => "eyeLookOutRight",
    EyeLookUpLeft => "eyeLookUpLeft",
    EyeLookUpRight => "eyeLookUpRight",
    EyeSquintLeft => "eyeSquintLeft",
    EyeSquintRight => "eyeSquintRight",
    EyeWideLeft => "eyeWideLeft",
    EyeWideRight => "eyeWideRight",
    JawForward => "jawForward",
    JawLeft => "jawLeft",
    JawOpen => "jawOpen",
    JawRight => "jawRight",
    MouthClose => "mouthClose",
    MouthDimpleLeft => "mouthDimpleLeft",
    MouthDimpleRight => "mouthDimpleRight",
    MouthFrownLeft => "mouthFrownLeft",
    MouthFrownRight => "mouthFrownRight",
    MouthFunnel => "mouthFunnel",
    MouthLeft => "mouthLeft",
    MouthLowerDownLeft => "mouthLowerDownLeft",
    MouthLowerDownRight => "mouthLowerDownRight",
    MouthPressLeft => "mouthPressLeft",
    MouthPressRight => "mouthPressRight",
    MouthPucker => "mouthPucker",
    MouthRight => "mouthRight",
    MouthRollLower => "mouthRollLower",
    MouthRollUpper => "mouthRollUpper",
    MouthShrugLower => "mouthShrugLower",
    MouthShrugUpper => "mouthShrugUpper",
    MouthSmileLeft => "mouthSmileLeft",
    MouthSmileRight => "mouthSmileRight",
    MouthStretchLeft => "mouthStretchLeft",
    MouthStretchRight => "mouthStretchRight",
    MouthUpperUpLeft => "mouthUpperUpLeft",
    MouthUpperUpRight => "mouthUpperUpRight",
    NoseSneerLeft => "noseSneerLeft",
    NoseSneerRight => "noseSneerRight",
}

/// The 52 coefficients of one face.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendshapeScores(pub [f32; 52]);

impl BlendshapeScores {
    pub fn get(&self, b: Blendshape) -> f32 {
        self.0[b.index()]
    }

    /// `(name, value)` pairs in model order.
    pub fn iter(&self) -> impl Iterator<Item = (Blendshape, f32)> + '_ {
        Blendshape::ALL.iter().map(|b| (*b, self.0[b.index()]))
    }
}

/// The blendshape model bound to a session.
#[derive(Debug, Clone)]
pub struct Blendshapes {
    session: Arc<ModelSession>,
}

impl Blendshapes {
    pub fn new(session: Arc<ModelSession>) -> Result<Blendshapes, ModelError> {
        let ok_in = session.inputs().len() == 1
            && session.inputs()[0].shape.len() == 3
            && session.inputs()[0].shape[1..] == [146, 2];
        let ok_out = session.outputs().len() == 1
            && session.outputs()[0]
                .shape
                .iter()
                .map(|d| (*d).max(1))
                .product::<i64>()
                == 52;
        if !(ok_in && ok_out) {
            return Err(ModelError::Spec {
                id: session.id().to_owned(),
                reason: "expected one [1,146,2] input and a 52-value output".to_owned(),
            });
        }
        Ok(Blendshapes { session })
    }

    pub fn session(&self) -> &Arc<ModelSession> {
        &self.session
    }

    /// The `[1, 146, 2]` input for a face's landmarks (pixels of the image they were computed on).
    pub fn input(landmarks: &[Point3]) -> Result<TensorF32, ModelError> {
        if landmarks.len() < 478 {
            return Err(ModelError::InvalidInput(format!(
                "blendshapes need 478 landmarks, got {}",
                landmarks.len()
            )));
        }
        let data = BLENDSHAPE_LANDMARKS
            .iter()
            .flat_map(|&i| [landmarks[i].x, landmarks[i].y])
            .collect();
        TensorF32::new(vec![1, 146, 2], data)
    }

    /// Run the model on prepared `[1, 146, 2]` points.
    pub fn predict_points(&self, points: &TensorF32) -> Result<BlendshapeScores, ModelError> {
        let out = self.session.run(std::slice::from_ref(points))?;
        let values: [f32; 52] = out[0].data().try_into().map_err(|_| ModelError::Spec {
            id: self.session.id().to_owned(),
            reason: "output does not have 52 values".to_owned(),
        })?;
        Ok(BlendshapeScores(values))
    }

    /// Coefficients for a face's 478 landmarks.
    pub fn predict(&self, landmarks: &[Point3]) -> Result<BlendshapeScores, ModelError> {
        self.predict_points(&Blendshapes::input(landmarks)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landmark_subset_and_names_match_mediapipe() {
        assert_eq!(BLENDSHAPE_LANDMARKS.len(), 146);
        assert!(BLENDSHAPE_LANDMARKS.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(BLENDSHAPE_LANDMARKS[145], 477);
        assert_eq!(Blendshape::ALL.len(), 52);
        assert_eq!(Blendshape::Neutral.index(), 0);
        assert_eq!(Blendshape::EyeBlinkLeft.index(), 9);
        assert_eq!(Blendshape::JawOpen.index(), 25);
        assert_eq!(Blendshape::MouthSmileRight.index(), 45);
        assert_eq!(Blendshape::NoseSneerRight.index(), 51);
        assert_eq!(Blendshape::EyeLookUpRight.name(), "eyeLookUpRight");
        let pts: Vec<Point3> = (0..478)
            .map(|i| Point3::new(i as f32, -(i as f32), 0.0))
            .collect();
        let t = Blendshapes::input(&pts).unwrap();
        assert_eq!(t.shape(), &[1, 146, 2]);
        assert_eq!(&t.data()[4..6], &[4.0, -4.0]);
        assert!(Blendshapes::input(&pts[..10]).is_err());
    }
}
