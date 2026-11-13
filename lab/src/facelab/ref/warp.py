"""MediaPipe ImageToTensor (OpenCV CPU path), reproduced.

Source: mediapipe/calculators/tensor/image_to_tensor_converter_opencv.cc and image_to_tensor_utils.cc.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import cv2
import numpy as np


@dataclass(frozen=True)
class RotatedRect:
    """ROI in source pixels; rotation in radians, clockwise-positive in image coordinates (y down)."""

    cx: float
    cy: float
    w: float
    h: float
    rotation: float = 0.0

    def to_json(self) -> dict:
        return {"cx": self.cx, "cy": self.cy, "w": self.w, "h": self.h, "rotation": self.rotation}


def whole_image(width: int, height: int) -> RotatedRect:
    return RotatedRect(width / 2, height / 2, float(width), float(height), 0.0)


def pad_roi(roi: RotatedRect, out_w: int, out_h: int) -> tuple[RotatedRect, tuple[float, float, float, float]]:
    """keep_aspect_ratio: enlarge the ROI to the tensor aspect; return it and (left, top, right, bottom) padding."""
    tensor_ar = out_h / out_w
    roi_ar = roi.h / roi.w
    if tensor_ar > roi_ar:
        new_w, new_h = roi.w, roi.w * tensor_ar
        v = (1.0 - roi_ar / tensor_ar) / 2.0
        pad = (0.0, v, 0.0, v)
    else:
        new_w, new_h = roi.h / tensor_ar, roi.h
        hpad = (1.0 - tensor_ar / roi_ar) / 2.0
        pad = (hpad, 0.0, hpad, 0.0)
    return RotatedRect(roi.cx, roi.cy, new_w, new_h, roi.rotation), pad


def warp_matrix(roi: RotatedRect, out_w: int, out_h: int) -> np.ndarray:
    """3x3 source->tensor projective matrix, exactly as MediaPipe builds it (corner-to-corner)."""
    box = cv2.RotatedRect((roi.cx, roi.cy), (roi.w, roi.h), roi.rotation * 180.0 / math.pi)
    src = cv2.boxPoints(box).astype(np.float32)  # bottom-left, top-left, top-right, bottom-right
    dst = np.float32([[0, out_h], [0, 0], [out_w, 0], [out_w, out_h]])
    return cv2.getPerspectiveTransform(src, dst)


def warp(rgb: np.ndarray, roi: RotatedRect, out_w: int, out_h: int, border: str) -> tuple[np.ndarray, np.ndarray]:
    """uint8 RGB tensor image (before normalisation) and the source->tensor matrix."""
    m = warp_matrix(roi, out_w, out_h)
    mode = cv2.BORDER_CONSTANT if border == "zero" else cv2.BORDER_REPLICATE
    out = cv2.warpPerspective(rgb, m, (out_w, out_h), flags=cv2.INTER_LINEAR, borderMode=mode, borderValue=0)
    return out, m


def normalise(img: np.ndarray, value_range: str) -> np.ndarray:
    x = img.astype(np.float32)
    x = x / 255.0 if value_range.startswith("x/255") else (x - 127.5) / 127.5
    return x[None]
