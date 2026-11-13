"""Reference image measures for the goldens (contracts/checks.yaml ``definitions``).

Luma (``definitions.luma``, adopted from OFIQ, image_utils.cpp GetLuminanceImageFromBGR):
    each 8-bit sRGB channel c/255 is linearised:  x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ^ 2.4
    Y = 0.2126 R + 0.7152 G + 0.0722 B   (BT.709 / sRGB primaries, linear light)
    L = floor(Y * 255 + 0.5)              (uint8)
Levels on that scale: underExposureLevel 25 (L <= 25), overExposureLevel 247 (L >= 247), as OFIQ's
UnderExposurePrevention / OverExposurePrevention ranges {0,25} and {247,255}.

Face-skin region (``definitions.faceSkinRegion``): pixels the segmenter classes face-skin (class 3) inside the
polygon of the landmark face oval (FACE_OVAL, below, in order).
"""

from __future__ import annotations

import cv2
import numpy as np

UNDER_EXPOSURE_LEVEL = 25
OVER_EXPOSURE_LEVEL = 247
FACE_SKIN_CLASS = 3

# MediaPipe face-mesh silhouette, in polygon order (FaceLandmarksConnections.FACE_LANDMARKS_FACE_OVAL).
FACE_OVAL = [
    10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377,
    152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234, 127, 162, 21, 54, 103, 67, 109,
]  # fmt: skip


def _srgb_to_linear(x: np.ndarray) -> np.ndarray:
    return np.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)


_LUT = _srgb_to_linear(np.arange(256) / 255.0)


def ofiq_luma(rgb: np.ndarray) -> np.ndarray:
    """uint8 luma image per definitions.luma (OFIQ)."""
    lin = _LUT[rgb.astype(np.intp)]
    y = 0.2126 * lin[..., 0] + 0.7152 * lin[..., 1] + 0.0722 * lin[..., 2]
    return np.floor(y * 255.0 + 0.5).astype(np.uint8)


def face_oval_mask(landmarks_px: np.ndarray, shape: tuple[int, int]) -> np.ndarray:
    """Boolean mask (H, W) of the filled face-oval polygon (landmark x, y in image px)."""
    poly = np.round(np.asarray(landmarks_px)[FACE_OVAL, :2]).astype(np.int32)
    m = np.zeros(shape, np.uint8)
    cv2.fillPoly(m, [poly], 1)
    return m.astype(bool)


def face_skin_region(
    landmarks_px: np.ndarray, category_mask: np.ndarray, roi: list[int], shape: tuple[int, int]
) -> np.ndarray:
    """definitions.faceSkinRegion: face-skin class inside the face oval. ``category_mask`` covers ``roi`` [x,y,w,h]."""
    x0, y0, w, h = roi
    skin = np.zeros(shape, bool)
    skin[y0 : y0 + h, x0 : x0 + w] = category_mask[:h, :w] == FACE_SKIN_CLASS
    return skin & face_oval_mask(landmarks_px, shape)


def illumination(rgb: np.ndarray, region: np.ndarray) -> dict:
    """LUMINANCE_MEAN (mean luma / 255), UNDER_EXPOSURE and OVER_EXPOSURE (fractions) over ``region``."""
    n = int(region.sum())
    if n == 0:
        return {"regionPixels": 0, "LUMINANCE_MEAN": None, "UNDER_EXPOSURE": None, "OVER_EXPOSURE": None}
    lum = ofiq_luma(rgb)[region]
    return {
        "regionPixels": n,
        "LUMINANCE_MEAN": round(float(lum.mean()) / 255.0, 5),
        "UNDER_EXPOSURE": round(float((lum <= UNDER_EXPOSURE_LEVEL).mean()), 5),
        "OVER_EXPOSURE": round(float((lum >= OVER_EXPOSURE_LEVEL).mean()), 5),
    }
