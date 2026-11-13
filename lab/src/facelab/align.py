"""ArcFace 5-point alignment (docs/architecture.md section 3.3) and the AuraFace embedding."""

from __future__ import annotations

import cv2
import numpy as np

from facelab.ref.pipeline import session

LANDMARKS5 = (468, 473, 1, 61, 291)  # image-left eye, image-right eye, nose tip, image-left / image-right mouth
TEMPLATE_112 = np.array(
    [[38.2946, 51.6963], [73.5318, 51.5014], [56.0252, 71.7366], [41.5493, 92.3655], [70.7299, 92.2041]],
    np.float64,
)


def umeyama_similarity(src: np.ndarray, dst: np.ndarray) -> np.ndarray:
    """Least-squares similarity (rotation, uniform scale, translation; no reflection) mapping src -> dst.

    Umeyama (1991), as in skimage.transform.SimilarityTransform.estimate used by InsightFace's estimate_norm.
    Returns the 2x3 matrix.
    """
    src = np.asarray(src, np.float64)
    dst = np.asarray(dst, np.float64)
    mu_s, mu_d = src.mean(0), dst.mean(0)
    s0, d0 = src - mu_s, dst - mu_d
    cov = d0.T @ s0 / len(src)
    u, sig, vt = np.linalg.svd(cov)
    d = np.ones(2)
    if np.linalg.det(u) * np.linalg.det(vt) < 0:
        d[-1] = -1
    rot = u @ np.diag(d) @ vt
    scale = (sig * d).sum() / s0.var(0).sum()
    t = mu_d - scale * rot @ mu_s
    return np.hstack([scale * rot, t[:, None]])


def aligned_crop(rgb: np.ndarray, points5: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    m = umeyama_similarity(points5, TEMPLATE_112)
    crop = cv2.warpAffine(rgb, m, (112, 112), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_CONSTANT, borderValue=0)
    return crop, m


def embed(crop_rgb: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """(raw 512 output, L2-normalised embedding) for a 112x112 RGB uint8 crop."""
    x = ((crop_rgb.astype(np.float32) - 127.5) / 127.5).transpose(2, 0, 1)[None]
    raw = session("auraface-v1").run(None, {"input": x})[0][0]
    return raw, raw / np.linalg.norm(raw)
