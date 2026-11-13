//! Strongly typed check identifiers. Must match `contracts/checks.yaml` exactly; a unit test in
//! `catalogue.rs` enforces this in both directions.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A check id that is not in the catalogue.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown check id `{0}`")]
pub struct UnknownCheckId(pub String);

macro_rules! check_ids {
    ($($variant:ident => $s:literal),+ $(,)?) => {
        /// Check identifier from the catalogue (`contracts/checks.yaml`).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum CheckId { $($variant),+ }

        impl CheckId {
            /// Every check id, in declaration order.
            pub const ALL: &'static [CheckId] = &[$(CheckId::$variant),+];

            /// The contract string, e.g. `"HEAD_POSE_YAW"`.
            pub const fn as_str(self) -> &'static str {
                match self { $(CheckId::$variant => $s),+ }
            }
        }

        impl FromStr for CheckId {
            type Err = UnknownCheckId;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($s => Ok(CheckId::$variant),)+
                    _ => Err(UnknownCheckId(s.to_owned())),
                }
            }
        }
    };
}

check_ids! {
    ImageResolution => "IMAGE_RESOLUTION",
    CompressionQuality => "COMPRESSION_QUALITY",
    FacePresent => "FACE_PRESENT",
    SingleFace => "SINGLE_FACE",
    InterEyeDistance => "INTER_EYE_DISTANCE",
    HeadInFrame => "HEAD_IN_FRAME",
    EyeMidpointHorizontal => "EYE_MIDPOINT_HORIZONTAL",
    EyeMidpointVertical => "EYE_MIDPOINT_VERTICAL",
    HeadWidthRatio => "HEAD_WIDTH_RATIO",
    HeadHeightRatio => "HEAD_HEIGHT_RATIO",
    HeadPoseYaw => "HEAD_POSE_YAW",
    HeadPosePitch => "HEAD_POSE_PITCH",
    HeadPoseRoll => "HEAD_POSE_ROLL",
    EyesVisible => "EYES_VISIBLE",
    EyesOpen => "EYES_OPEN",
    GazeFrontal => "GAZE_FRONTAL",
    MouthClosed => "MOUTH_CLOSED",
    ExpressionNeutral => "EXPRESSION_NEUTRAL",
    FaceOcclusion => "FACE_OCCLUSION",
    HeadCovering => "HEAD_COVERING",
    Eyeglasses => "EYEGLASSES",
    LuminanceMean => "LUMINANCE_MEAN",
    UnderExposure => "UNDER_EXPOSURE",
    OverExposure => "OVER_EXPOSURE",
    DynamicRange => "DYNAMIC_RANGE",
    IlluminationUniformity => "ILLUMINATION_UNIFORMITY",
    Sharpness => "SHARPNESS",
    BackgroundUniformity => "BACKGROUND_UNIFORMITY",
    DocumentDetected => "DOCUMENT_DETECTED",
    PortraitFound => "PORTRAIT_FOUND",
    ProbeReferenceDuplicate => "PROBE_REFERENCE_DUPLICATE",
    ProbeDocumentPresent => "PROBE_DOCUMENT_PRESENT",
    ProbeScreenArtifacts => "PROBE_SCREEN_ARTIFACTS",
}

impl fmt::Display for CheckId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for CheckId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CheckId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_strings() {
        for id in CheckId::ALL {
            assert_eq!(id.as_str().parse::<CheckId>().unwrap(), *id);
        }
        assert!("NOT_A_CHECK".parse::<CheckId>().is_err());
    }
}
