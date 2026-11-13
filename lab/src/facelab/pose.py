"""Reference head pose (contracts/checks.yaml ``definitions.pose``).

**Primary (the contract): weak-perspective weighted Procrustes fit.** :func:`fit_weak_perspective`

1. Landmark frame: from the 478 landmarks in image pixels (x right, y down; z = MediaPipe depth scaled like x,
   i.e. z_norm * width of the image the landmarker saw, z < 0 closer), build P_i = (x_i, -y_i, -z_i).
   This is right-handed with X right, Y up, Z toward the camera, like the canonical model.
2. Canonical points C_i: vertex i of mp-canonical-face (cm, X right, Y up, Z toward the camera).
3. Weights w_i: the asset's 33 ``procrustesLandmarkBasis`` weights; all other landmarks weigh 0. Weights are
   used as given (no renormalisation is needed: only their ratios matter).
4. Solve min over s > 0, R in SO(3), t of  sum_i w_i * || s R C_i + t - P_i ||^2  (Umeyama 1991, weighted):
     mu_C = sum w C / sum w,  mu_P = sum w P / sum w,  C' = C - mu_C,  P' = P - mu_P
     S = sum_i w_i P'_i C'_i^T / sum w            (3x3 cross-covariance)
     U, D, V^T = svd(S);  E = diag(1, 1, sign(det(U V^T)))
     R = U E V^T;  s = trace(D E) / (sum_i w_i |C'_i|^2 / sum w);  t = mu_P - s R mu_C
   Orthographic with one uniform scale s (px per cm): no focal length, no principal point, so the result does
   not depend on where the face sits in the frame. R maps canonical directions to the observed frame;
   identity = frontal.
5. Angles: R = Rz(c) . Ry(b) . Rx(a) (intrinsic z-y-x); yaw = +b, pitch = -a, roll = -c (> 0: face points to
   image-right / up / is rotated clockwise in the image).

**Informational only:** :func:`pose_from_matrix` on MediaPipe's perspective ``facial_transformation_matrixes``.
That matrix assumes a ~63 degree vertical field of view, so its pitch picks up roughly a third of the face's
line-of-sight angle: the same face reads differently at the top and bottom of a frame.
"""

from __future__ import annotations

import json
import math
from functools import cache

import numpy as np

from facelab.paths import MODELS_DIR


def euler_zyx(rotation: np.ndarray) -> tuple[float, float, float]:
    """(a, b, c) in degrees with R = Rz(c) @ Ry(b) @ Rx(a). b is clamped to [-90, 90]."""
    r = np.asarray(rotation, np.float64)[:3, :3]
    b = math.asin(max(-1.0, min(1.0, -r[2, 0])))
    if abs(math.cos(b)) < 1e-9:  # gimbal lock: put everything in a
        a, c = math.atan2(-r[1, 2], r[1, 1]), 0.0
    else:
        a, c = math.atan2(r[2, 1], r[2, 2]), math.atan2(r[1, 0], r[0, 0])
    return math.degrees(a), math.degrees(b), math.degrees(c)


def pose_from_matrix(matrix: np.ndarray) -> dict[str, float]:
    a, b, c = euler_zyx(matrix)
    return {"yaw": b, "pitch": -a, "roll": -c}


def matrix_from_pose(yaw: float, pitch: float, roll: float) -> np.ndarray:
    """Inverse of :func:`pose_from_matrix` (3x3), for tests."""
    a, b, c = (math.radians(v) for v in (-pitch, yaw, -roll))
    rx = np.array([[1, 0, 0], [0, math.cos(a), -math.sin(a)], [0, math.sin(a), math.cos(a)]])
    ry = np.array([[math.cos(b), 0, math.sin(b)], [0, 1, 0], [-math.sin(b), 0, math.cos(b)]])
    rz = np.array([[math.cos(c), -math.sin(c), 0], [math.sin(c), math.cos(c), 0], [0, 0, 1]])
    return rz @ ry @ rx


@cache
def canonical() -> tuple[np.ndarray, np.ndarray]:
    """(468x3 canonical vertices, 478 weights with zeros outside the 33-landmark basis)."""
    doc = json.loads((MODELS_DIR / "mp-canonical-face.json").read_text(encoding="utf-8"))
    verts = np.asarray(doc["vertices"], np.float64)
    w = np.zeros(478)
    for b in doc["procrustesLandmarkBasis"]:
        w[b["landmark"]] = b["weight"]
    return verts, w


def weighted_similarity(src: np.ndarray, dst: np.ndarray, w: np.ndarray) -> tuple[float, np.ndarray, np.ndarray]:
    """(s, R, t) minimising sum w_i |s R src_i + t - dst_i|^2 (weighted Umeyama, rotation in SO(3))."""
    w = np.asarray(w, np.float64)
    ws = w.sum()
    mu_s = (w[:, None] * src).sum(0) / ws
    mu_d = (w[:, None] * dst).sum(0) / ws
    s0, d0 = src - mu_s, dst - mu_d
    cov = (w[:, None, None] * d0[:, :, None] * s0[:, None, :]).sum(0) / ws
    u, sig, vt = np.linalg.svd(cov)
    e = np.diag([1.0, 1.0, np.sign(np.linalg.det(u @ vt)) or 1.0])
    rot = u @ e @ vt
    var_s = (w * (s0**2).sum(1)).sum() / ws
    scale = float(np.trace(np.diag(sig) @ e) / var_s)
    t = mu_d - scale * rot @ mu_s
    return scale, rot, t


def fit_weak_perspective(landmarks_px: np.ndarray) -> dict:
    """Pose per the contract from 478 x (x, y, z) image-pixel landmarks. Returns angles, R, scale, residual."""
    verts, w = canonical()
    lm = np.asarray(landmarks_px, np.float64)
    p = np.column_stack([lm[:468, 0], -lm[:468, 1], -lm[:468, 2]])
    scale, rot, t = weighted_similarity(verts, p, w[:468])
    resid = np.sqrt((w[:468] * ((scale * verts @ rot.T + t - p) ** 2).sum(1)).sum() / w[:468].sum())
    return {**pose_from_matrix(rot), "rotation": rot, "scale": scale, "weightedRmsResidualPx": float(resid)}
