"""ONNX models + reference processing, end to end (detector, landmarks)."""

from __future__ import annotations

from functools import cache

import numpy as np
import onnxruntime as ort

from facelab.paths import MODELS_DIR
from facelab.ref import blazeface
from facelab.ref.warp import RotatedRect, normalise, warp

DETECTOR_IDS = {"short": "mp-face-detector-short", "full": "mp-face-detector-full"}


@cache
def session(model_id: str) -> ort.InferenceSession:
    so = ort.SessionOptions()
    so.intra_op_num_threads = 1
    return ort.InferenceSession(str(MODELS_DIR / f"{model_id}.onnx"), so, providers=["CPUExecutionProvider"])


def detect(rgb: np.ndarray, variant: str, min_score: float = 0.5) -> tuple[list[blazeface.Detection], dict]:
    h, w = rgb.shape[:2]
    size = blazeface.CONFIGS[variant][0]
    roi, pad = blazeface.letterbox_roi(w, h, variant)
    tensor_img, m = warp(rgb, roi, size, size, border="zero")
    reg, logits = session(DETECTOR_IDS[variant]).run(None, {"input": normalise(tensor_img, "(x")})
    rows = blazeface.weighted_nms(blazeface.decode(reg, logits, variant, min_score), 0.3)
    dets = [blazeface.to_image(r, w, h, pad) for r in rows]
    io = {
        "tensorImage": tensor_img,
        "roi": roi,
        "padding": pad,
        "matrix": m,
        "regressors": reg,
        "classificators": logits,
    }
    return dets, io


def landmarks(rgb: np.ndarray, roi: RotatedRect) -> tuple[np.ndarray, float, dict]:
    """478x3 landmarks in image px (z in the same px scale), presence probability, and the raw tensors."""
    tensor_img, m = warp(rgb, roi, 256, 256, border="replicate")
    lm, presence_logit, tongue = session("mp-face-landmarker").run(None, {"input": normalise(tensor_img, "x/255")})
    pts = lm.reshape(478, 3).astype(np.float64) / 256.0  # normalised within the ROI
    # Project: ROI-normalised -> image px (LandmarkProjectionCalculator).
    c, s = np.cos(roi.rotation), np.sin(roi.rotation)
    dx, dy = (pts[:, 0] - 0.5) * roi.w, (pts[:, 1] - 0.5) * roi.h
    x = roi.cx + dx * c - dy * s
    y = roi.cy + dx * s + dy * c
    z = pts[:, 2] * roi.w
    presence = float(1.0 / (1.0 + np.exp(-presence_logit.ravel()[0])))
    io = {"tensorImage": tensor_img, "roi": roi, "matrix": m, "landmarks": lm, "presence_logit": presence_logit,
          "tongue_out": tongue}  # fmt: skip
    return np.stack([x, y, z], 1), presence, io
