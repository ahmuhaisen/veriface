//! Head pose per `contracts/checks.yaml` `definitions.pose`: a weighted, scaled-orthographic
//! (weak-perspective) rigid Procrustes fit of the landmarks to the canonical face.
//!
//! 1. Landmark frame: `P_i = (x_i, -y_i, -z_i)` for the 468 mesh landmarks (image pixels; z is the
//!    landmarker depth in the same scale, MediaPipe z < 0 = closer), i.e. X right, Y up, Z towards
//!    the camera, like the canonical model.
//! 2. Weighted similarity fit (Umeyama 1991) of the canonical vertices `C_i` (asset
//!    `mp-canonical-face`) to `P_i` with the asset's 33 weights: minimise
//!    `sum w_i |s R C_i + t - P_i|^2` over `s > 0`, `R` in SO(3), `t`. One uniform scale, no focal
//!    length and no principal point, so the pose does not depend on where the face sits in the
//!    frame (MediaPipe's perspective `facial_transformation_matrix` is not used).
//! 3. `R = Rz(c) . Ry(b) . Rx(a)` (intrinsic z-y-x); yaw = +b, pitch = -a, roll = -c.
//! 4. [`PitchOffset`] is added to pitch: a versioned, model-specific correction, 0 until the lab
//!    measures it.
//!
//! The lab's Python `fit_weak_perspective` (`lab/src/facelab/pose.py`) is the reference.

use face_models::{NUM_LANDMARKS, Point3};

use crate::error::QualityError;
use crate::linalg::{self, Mat3};

/// Number of mesh landmarks in the canonical face (no irises).
pub const MESH_LANDMARKS: usize = 468;

/// Head pose in degrees with the signs of `contracts/checks.yaml`: yaw > 0 = face points towards
/// image-right, pitch > 0 = face points up, roll > 0 = rotated clockwise in the image.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
}

/// A model-specific correction added to raw pitch, with the calibration it came from. Applied in
/// one place ([`PoseEstimator::fit`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PitchOffset {
    /// Landmark model the offset was measured for.
    pub model: &'static str,
    /// Calibration version (e.g. the lab's L7 report id).
    pub version: &'static str,
    /// Degrees added to the raw pitch.
    pub degrees: f32,
}

/// The pitch offset in force. Raw MediaPipe pitch reads about -8 degrees on frontal portraits
/// (lab: mean -7.6, median -9.1 on 18 NASA portraits); the lab will measure the model offset (L7).
/// Until then no correction is applied.
pub const PITCH_OFFSET: PitchOffset = PitchOffset {
    model: "mp-face-landmarker",
    version: "uncalibrated",
    degrees: 0.0,
};

/// MediaPipe's canonical face: 468 vertices (cm) and the Procrustes landmark weights.
#[derive(Clone, Debug, PartialEq)]
pub struct CanonicalFace {
    vertices: Vec<[f64; 3]>,
    /// `(landmark index, weight)` with weight > 0.
    weights: Vec<(usize, f64)>,
}

impl CanonicalFace {
    /// Parse the `mp-canonical-face` asset (JSON: `vertices` = 468 x [x, y, z],
    /// `procrustesLandmarkBasis` = [{landmark, weight}]).
    pub fn from_json(bytes: &[u8]) -> Result<CanonicalFace, QualityError> {
        let bad = |reason: String| QualityError::Asset {
            id: face_models::ids::CANONICAL_FACE,
            reason,
        };
        let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        let vertices: Vec<[f64; 3]> = v["vertices"]
            .as_array()
            .ok_or_else(|| bad("no `vertices` array".to_owned()))?
            .iter()
            .map(|p| {
                let a = p.as_array().filter(|a| a.len() == 3)?;
                Some([a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?])
            })
            .collect::<Option<_>>()
            .ok_or_else(|| bad("vertices must be [x, y, z] numbers".to_owned()))?;
        if vertices.len() != MESH_LANDMARKS {
            return Err(bad(format!(
                "{} vertices, expected {MESH_LANDMARKS}",
                vertices.len()
            )));
        }
        let weights: Vec<(usize, f64)> = v["procrustesLandmarkBasis"]
            .as_array()
            .ok_or_else(|| bad("no `procrustesLandmarkBasis` array".to_owned()))?
            .iter()
            .map(|e| {
                let i = usize::try_from(e["landmark"].as_u64()?).ok()?;
                let w = e["weight"].as_f64()?;
                (i < MESH_LANDMARKS && w >= 0.0 && w.is_finite()).then_some((i, w))
            })
            .collect::<Option<_>>()
            .ok_or_else(|| {
                bad(
                    "procrustesLandmarkBasis entries must be {landmark < 468, weight >= 0}"
                        .to_owned(),
                )
            })?;
        CanonicalFace::new(vertices, weights)
    }

    /// From vertices and `(landmark, weight)` pairs.
    pub fn new(
        vertices: Vec<[f64; 3]>,
        weights: Vec<(usize, f64)>,
    ) -> Result<CanonicalFace, QualityError> {
        let total: f64 = weights.iter().map(|(_, w)| w).sum();
        if vertices.len() != MESH_LANDMARKS || weights.len() < 3 || total.is_nan() || total <= 1e-9
        {
            return Err(QualityError::Asset {
                id: face_models::ids::CANONICAL_FACE,
                reason: "need 468 vertices and at least 3 positive weights".to_owned(),
            });
        }
        Ok(CanonicalFace {
            vertices,
            weights: weights.into_iter().filter(|(_, w)| *w > 0.0).collect(),
        })
    }

    pub fn vertices(&self) -> &[[f64; 3]] {
        &self.vertices
    }

    pub fn weights(&self) -> &[(usize, f64)] {
        &self.weights
    }
}

/// The result of a pose fit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoseFit {
    /// Rotation mapping canonical directions into the landmark frame (identity = frontal).
    pub rotation: Mat3,
    /// Pixels per canonical centimetre.
    pub scale: f64,
    /// Translation in the landmark frame (pixels).
    pub translation: [f64; 3],
    /// Weighted RMS residual of the fit, pixels.
    pub residual_px: f64,
    /// `(a, b, c)` in degrees with `R = Rz(c) . Ry(b) . Rx(a)`.
    pub euler_raw: [f64; 3],
    /// Contract-signed angles, pitch corrected by the [`PitchOffset`].
    pub pose: Pose,
}

/// Decompose a rotation as `R = Rz(c) . Ry(b) . Rx(a)`; degrees. Identical to the lab reference,
/// including the gimbal-lock branch.
pub fn euler_zyx(r: &Mat3) -> [f64; 3] {
    let b = (-r[2][0]).clamp(-1.0, 1.0).asin();
    let (a, c) = if b.cos().abs() < 1e-9 {
        ((-r[1][2]).atan2(r[1][1]), 0.0)
    } else {
        (r[2][1].atan2(r[2][2]), r[1][0].atan2(r[0][0]))
    };
    [a.to_degrees(), b.to_degrees(), c.to_degrees()]
}

/// Contract-signed pose from MediaPipe's raw Euler angles (no offset).
pub fn pose_from_euler(euler: [f64; 3]) -> Pose {
    Pose {
        yaw: euler[1] as f32,
        pitch: -euler[0] as f32,
        roll: -euler[2] as f32,
    }
}

/// The rotation for a contract-signed pose: `R = Rz(-roll) . Ry(yaw) . Rx(-pitch)` in MediaPipe's
/// frame (inverse of [`euler_zyx`] + [`pose_from_euler`]).
pub fn rotation_from_pose(pose: Pose) -> Mat3 {
    let (a, b, c) = (
        f64::from(-pose.pitch).to_radians(),
        f64::from(pose.yaw).to_radians(),
        f64::from(-pose.roll).to_radians(),
    );
    let rx = [
        [1.0, 0.0, 0.0],
        [0.0, a.cos(), -a.sin()],
        [0.0, a.sin(), a.cos()],
    ];
    let ry = [
        [b.cos(), 0.0, b.sin()],
        [0.0, 1.0, 0.0],
        [-b.sin(), 0.0, b.cos()],
    ];
    let rz = [
        [c.cos(), -c.sin(), 0.0],
        [c.sin(), c.cos(), 0.0],
        [0.0, 0.0, 1.0],
    ];
    linalg::mul(&linalg::mul(&rz, &ry), &rx)
}

/// Head pose from face landmarks.
#[derive(Clone, Debug)]
pub struct PoseEstimator {
    canonical: CanonicalFace,
    pitch_offset: PitchOffset,
}

impl PoseEstimator {
    pub fn new(canonical: CanonicalFace) -> PoseEstimator {
        PoseEstimator {
            canonical,
            pitch_offset: PITCH_OFFSET,
        }
    }

    /// Use a different pitch offset than [`PITCH_OFFSET`].
    pub fn with_pitch_offset(mut self, offset: PitchOffset) -> PoseEstimator {
        self.pitch_offset = offset;
        self
    }

    pub fn pitch_offset(&self) -> PitchOffset {
        self.pitch_offset
    }

    pub fn canonical(&self) -> &CanonicalFace {
        &self.canonical
    }

    /// Fit `landmarks` (at least the 468 mesh points, image pixels, z in the x scale). `None` when
    /// the landmarks are degenerate.
    pub fn fit(&self, landmarks: &[Point3]) -> Option<PoseFit> {
        if landmarks.len() < MESH_LANDMARKS {
            return None;
        }
        let target: Vec<[f64; 3]> = landmarks[..MESH_LANDMARKS]
            .iter()
            .map(|p| [f64::from(p.x), -f64::from(p.y), -f64::from(p.z)])
            .collect();
        if !target.iter().flatten().all(|v| v.is_finite()) {
            return None;
        }
        let (scale, rotation, translation) = self.solve(&target)?;
        // Weighted RMS residual.
        let (mut sum, mut total) = (0.0f64, 0.0f64);
        for (i, w) in &self.canonical.weights {
            let p = linalg::apply(&rotation, self.canonical.vertices[*i]);
            let d: f64 = (0..3)
                .map(|k| (scale * p[k] + translation[k] - target[*i][k]).powi(2))
                .sum();
            sum += w * d;
            total += w;
        }
        let euler_raw = euler_zyx(&rotation);
        let mut pose = pose_from_euler(euler_raw);
        pose.pitch += self.pitch_offset.degrees;
        Some(PoseFit {
            rotation,
            scale,
            translation,
            residual_px: (sum / total).sqrt(),
            euler_raw,
            pose,
        })
    }

    /// Weighted similarity (Umeyama): `(s, R, t)` minimising `sum w_i |s R C_i + t - P_i|^2`.
    fn solve(&self, target: &[[f64; 3]]) -> Option<(f64, Mat3, [f64; 3])> {
        let src = &self.canonical.vertices;
        let weights = &self.canonical.weights;
        let total: f64 = weights.iter().map(|(_, w)| w).sum();
        let (mut mu_s, mut mu_d) = ([0.0f64; 3], [0.0f64; 3]);
        for (i, w) in weights {
            for k in 0..3 {
                mu_s[k] += w * src[*i][k];
                mu_d[k] += w * target[*i][k];
            }
        }
        let (mu_s, mu_d) = (mu_s.map(|v| v / total), mu_d.map(|v| v / total));
        let mut cov = [[0.0f64; 3]; 3];
        let mut var_s = 0.0f64;
        for (i, w) in weights {
            let s0 = [
                src[*i][0] - mu_s[0],
                src[*i][1] - mu_s[1],
                src[*i][2] - mu_s[2],
            ];
            let d0 = [
                target[*i][0] - mu_d[0],
                target[*i][1] - mu_d[1],
                target[*i][2] - mu_d[2],
            ];
            for (r, row) in cov.iter_mut().enumerate() {
                for (c, v) in row.iter_mut().enumerate() {
                    *v += w * d0[r] * s0[c] / total;
                }
            }
            var_s += w * (s0[0] * s0[0] + s0[1] * s0[1] + s0[2] * s0[2]) / total;
        }
        let rotation = linalg::nearest_rotation(&cov)?;
        // trace(D E) = trace(R^T cov) for the optimal R.
        let rt = linalg::transpose(&rotation);
        let trace: f64 = (0..3)
            .map(|k| (0..3).map(|j| rt[k][j] * cov[j][k]).sum::<f64>())
            .sum();
        if var_s.is_nan() || var_s <= 1e-12 || trace.is_nan() || trace <= 1e-12 {
            return None;
        }
        let scale = trace / var_s;
        let r_mu = linalg::apply(&rotation, mu_s);
        let translation = [
            mu_d[0] - scale * r_mu[0],
            mu_d[1] - scale * r_mu[1],
            mu_d[2] - scale * r_mu[2],
        ];
        Some((scale, rotation, translation))
    }

    /// The canonical face posed by `pose` (orthographic, `px_per_cm`), centred at `center_px`, as
    /// 478 landmarks (irises filled from the eye contours). For tests and synthetic fixtures.
    pub fn synthesize_landmarks(
        &self,
        pose: Pose,
        center_px: (f64, f64),
        px_per_cm: f64,
    ) -> Vec<Point3> {
        let r = rotation_from_pose(Pose {
            pitch: pose.pitch - self.pitch_offset.degrees,
            ..pose
        });
        let mut out: Vec<Point3> = self
            .canonical
            .vertices
            .iter()
            .map(|v| {
                let p = linalg::apply(&r, *v);
                Point3::new(
                    (center_px.0 + px_per_cm * p[0]) as f32,
                    (center_px.1 - px_per_cm * p[1]) as f32,
                    (-px_per_cm * p[2]) as f32,
                )
            })
            .collect();
        let centroid = |idx: &[usize]| {
            let n = idx.len() as f32;
            let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
            for &i in idx {
                x += out[i].x;
                y += out[i].y;
                z += out[i].z;
            }
            Point3::new(x / n, y / n, z / n)
        };
        let right = centroid(&face_models::mesh::RIGHT_EYE);
        let left = centroid(&face_models::mesh::LEFT_EYE);
        while out.len() < NUM_LANDMARKS {
            let p = if out.len() < 473 { right } else { left };
            out.push(p);
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The real canonical face asset from `models/`, if present.
    pub(crate) fn canonical() -> Option<CanonicalFace> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../models/mp-canonical-face.json");
        let bytes = std::fs::read(path).ok()?;
        Some(CanonicalFace::from_json(&bytes).unwrap())
    }

    /// A synthetic but non-degenerate canonical face (for when the asset is absent).
    pub(crate) fn synthetic_canonical() -> CanonicalFace {
        let vertices: Vec<[f64; 3]> = (0..MESH_LANDMARKS)
            .map(|i| {
                let t = i as f64 * 0.137;
                [
                    6.0 * t.sin(),
                    8.0 * (1.7 * t).cos(),
                    3.0 + 2.0 * (0.61 * t).sin(),
                ]
            })
            .collect();
        let weights = (0..33).map(|k| (k * 13 + 4, 0.03)).collect();
        CanonicalFace::new(vertices, weights).unwrap()
    }

    fn estimator() -> PoseEstimator {
        PoseEstimator::new(canonical().unwrap_or_else(synthetic_canonical))
    }

    #[test]
    fn decomposition_matches_the_lab_reference_and_round_trips() {
        for pose in [
            Pose {
                yaw: 0.0,
                pitch: 0.0,
                roll: 0.0,
            },
            Pose {
                yaw: 20.0,
                pitch: -10.0,
                roll: 5.0,
            },
            Pose {
                yaw: -35.0,
                pitch: 25.0,
                roll: -12.0,
            },
        ] {
            let r = rotation_from_pose(pose);
            let back = pose_from_euler(euler_zyx(&r));
            assert!((back.yaw - pose.yaw).abs() < 1e-4, "{pose:?} {back:?}");
            assert!((back.pitch - pose.pitch).abs() < 1e-4, "{pose:?} {back:?}");
            assert!((back.roll - pose.roll).abs() < 1e-4, "{pose:?} {back:?}");
        }
        // Gimbal lock (|b| = 90 degrees): everything goes into a, c = 0.
        let r = rotation_from_pose(Pose {
            yaw: 90.0,
            pitch: 0.0,
            roll: 0.0,
        });
        assert!((euler_zyx(&r)[1] - 90.0).abs() < 1e-6);
    }

    #[test]
    fn signs_follow_the_contract() {
        // Yaw > 0: the face's forward axis (+z) turns towards +x, i.e. image-right.
        let r = rotation_from_pose(Pose {
            yaw: 30.0,
            pitch: 0.0,
            roll: 0.0,
        });
        assert!(r[0][2] > 0.4);
        // Pitch > 0: forward axis gets +y (up).
        let r = rotation_from_pose(Pose {
            yaw: 0.0,
            pitch: 30.0,
            roll: 0.0,
        });
        assert!(r[1][2] > 0.4);
        // Roll > 0 (clockwise in the image): the face's up axis (+y) tilts towards +x (image-right).
        let r = rotation_from_pose(Pose {
            yaw: 0.0,
            pitch: 0.0,
            roll: 30.0,
        });
        assert!(r[0][1] > 0.4);
    }

    #[test]
    fn procrustes_recovers_known_poses_exactly() {
        let est = estimator();
        for pose in [
            Pose {
                yaw: 0.0,
                pitch: 0.0,
                roll: 0.0,
            },
            Pose {
                yaw: 15.0,
                pitch: 0.0,
                roll: 0.0,
            },
            Pose {
                yaw: -20.0,
                pitch: 10.0,
                roll: 0.0,
            },
            Pose {
                yaw: 5.0,
                pitch: -12.0,
                roll: 8.0,
            },
            Pose {
                yaw: 0.0,
                pitch: 0.0,
                roll: -15.0,
            },
            Pose {
                yaw: 40.0,
                pitch: 25.0,
                roll: 30.0,
            },
        ] {
            let lm = est.synthesize_landmarks(pose, (400.0, 300.0), 8.0);
            let fit = est.fit(&lm).unwrap();
            let p = fit.pose;
            assert!((p.yaw - pose.yaw).abs() < 1e-3, "{pose:?} -> {p:?}");
            assert!((p.pitch - pose.pitch).abs() < 1e-3, "{pose:?} -> {p:?}");
            assert!((p.roll - pose.roll).abs() < 1e-3, "{pose:?} -> {p:?}");
            assert!((fit.scale - 8.0).abs() < 1e-3, "scale {}", fit.scale);
            assert!(fit.residual_px < 1e-3);
        }
    }

    #[test]
    fn pose_does_not_depend_on_position_or_scale() {
        let est = estimator();
        let pose = Pose {
            yaw: 12.0,
            pitch: -7.0,
            roll: 4.0,
        };
        let a = est
            .fit(&est.synthesize_landmarks(pose, (100.0, 80.0), 5.0))
            .unwrap()
            .pose;
        let b = est
            .fit(&est.synthesize_landmarks(pose, (1900.0, 1500.0), 20.0))
            .unwrap()
            .pose;
        assert!(
            (a.yaw - b.yaw).abs() < 1e-3
                && (a.pitch - b.pitch).abs() < 1e-3
                && (a.roll - b.roll).abs() < 1e-3
        );
    }

    #[test]
    fn image_semantics_of_the_signs() {
        let Some(canon) = canonical() else { return };
        let est = PoseEstimator::new(canon);
        // Turned towards image-right: the nose tip lies right of the eye midpoint.
        let lm = est.synthesize_landmarks(
            Pose {
                yaw: 25.0,
                pitch: 0.0,
                roll: 0.0,
            },
            (400.0, 300.0),
            8.0,
        );
        let eyes = (lm[468].x + lm[473].x) * 0.5;
        assert!(lm[1].x > eyes, "nose {} eyes {eyes}", lm[1].x);
        // Pitched up: the nose tip moves up relative to the eyes (smaller y gap).
        let up = est.synthesize_landmarks(
            Pose {
                yaw: 0.0,
                pitch: 20.0,
                roll: 0.0,
            },
            (400.0, 300.0),
            8.0,
        );
        let level = est.synthesize_landmarks(Pose::default(), (400.0, 300.0), 8.0);
        assert!(up[1].y - up[468].y < level[1].y - level[468].y);
        // Rolled clockwise: the image-right eye is lower (larger y).
        let lm = est.synthesize_landmarks(
            Pose {
                yaw: 0.0,
                pitch: 0.0,
                roll: 15.0,
            },
            (400.0, 300.0),
            8.0,
        );
        assert!(lm[473].y > lm[468].y);
        // A horizontal flip (with mirrored landmark identities) negates yaw and roll, keeps pitch.
        let base = Pose {
            yaw: 12.0,
            pitch: -6.0,
            roll: 7.0,
        };
        let lm = est.synthesize_landmarks(base, (400.0, 300.0), 8.0);
        let flipped: Vec<Point3> = lm
            .iter()
            .map(|p| Point3::new(800.0 - p.x, p.y, p.z))
            .collect();
        let f = est.fit(&mirror_indices(&flipped)).unwrap().pose;
        assert!(
            (f.yaw + base.yaw).abs() < 0.05 && (f.roll + base.roll).abs() < 0.05,
            "{f:?}"
        );
        assert!((f.pitch - base.pitch).abs() < 0.05, "{f:?}");
    }

    /// Re-index a mirrored landmark set using the canonical face's left-right symmetry: each vertex
    /// maps to the vertex nearest its x-mirror; iris rings swap.
    fn mirror_indices(pts: &[Point3]) -> Vec<Point3> {
        let canon = canonical().unwrap();
        let v = canon.vertices();
        let partner: Vec<usize> = (0..MESH_LANDMARKS)
            .map(|i| {
                let m = [-v[i][0], v[i][1], v[i][2]];
                (0..MESH_LANDMARKS)
                    .min_by(|&a, &b| {
                        let da: f64 = (0..3).map(|k| (v[a][k] - m[k]).powi(2)).sum();
                        let db: f64 = (0..3).map(|k| (v[b][k] - m[k]).powi(2)).sum();
                        da.total_cmp(&db)
                    })
                    .unwrap()
            })
            .collect();
        let mut out: Vec<Point3> = (0..MESH_LANDMARKS).map(|i| pts[partner[i]]).collect();
        out.push(pts[473]);
        out.extend_from_slice(&pts[474..478]);
        out.push(pts[468]);
        out.extend_from_slice(&pts[469..473]);
        out
    }

    #[test]
    fn pitch_offset_is_applied_in_one_place() {
        let base = estimator();
        let lm = base.synthesize_landmarks(Pose::default(), (400.0, 300.0), 8.0);
        let p0 = base.fit(&lm).unwrap().pose.pitch;
        let shifted = base.clone().with_pitch_offset(PitchOffset {
            model: "test",
            version: "t1",
            degrees: 7.5,
        });
        let p1 = shifted.fit(&lm).unwrap().pose.pitch;
        assert!((p1 - p0 - 7.5).abs() < 1e-4);
        assert_eq!(PITCH_OFFSET.degrees, 0.0);
    }

    #[test]
    fn degenerate_input_gives_none() {
        let est = estimator();
        let same = vec![Point3::new(10.0, 10.0, 0.0); 478];
        assert!(est.fit(&same).is_none());
        assert!(est.fit(&same[..100]).is_none());
        let canon_json = br#"{"vertices": [[0,0,0]], "procrustesLandmarkBasis": []}"#;
        assert!(CanonicalFace::from_json(canon_json).is_err());
    }
}
