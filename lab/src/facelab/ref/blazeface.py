"""BlazeFace anchors, decoding and weighted NMS (MediaPipe SsdAnchors / TensorsToDetections / NMS)."""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

from facelab.ref.warp import RotatedRect, pad_roi, whole_image

CONFIGS = {
    # variant: (input size, strides, interpolated_scale_aspect_ratio)
    "short": (128, [8, 16, 16, 16], 1.0),
    "full": (192, [4], 0.0),
}


@dataclass
class Detection:
    """Image-pixel box (xmin, ymin, w, h), score, 6 keypoints (x, y) in image pixels."""

    box: tuple[float, float, float, float]
    score: float
    keypoints: list[tuple[float, float]]

    def to_json(self) -> dict:
        return {
            "box": [round(v, 3) for v in self.box],
            "score": round(self.score, 6),
            "keypoints": [[round(x, 3), round(y, 3)] for x, y in self.keypoints],
        }


def anchors(variant: str) -> np.ndarray:
    """(N, 2) anchor centres in [0,1]; fixed_anchor_size so w = h = 1."""
    size, strides, interp = CONFIGS[variant]
    out = []
    layer = 0
    while layer < len(strides):
        per_cell = 0
        last = layer
        while last < len(strides) and strides[last] == strides[layer]:
            per_cell += 1 + (1 if interp > 0 else 0)  # aspect_ratios [1.0] + optional interpolated anchor
            last += 1
        fm = math.ceil(size / strides[layer])
        for y in range(fm):
            for x in range(fm):
                for _ in range(per_cell):
                    out.append(((x + 0.5) / fm, (y + 0.5) / fm))
        layer = last
    return np.asarray(out, np.float32)


def decode(regressors: np.ndarray, logits: np.ndarray, variant: str, min_score: float) -> list[np.ndarray]:
    """Rows of [score, xmin, ymin, xmax, ymax, kx0, ky0, ...] in letterboxed-tensor-normalised coordinates."""
    size = CONFIGS[variant][0]
    a = anchors(variant)
    r = regressors.reshape(-1, 16).astype(np.float64)
    s = 1.0 / (1.0 + np.exp(-np.clip(logits.reshape(-1).astype(np.float64), -100.0, 100.0)))
    cx = r[:, 0] / size + a[:, 0]
    cy = r[:, 1] / size + a[:, 1]
    w = r[:, 2] / size
    h = r[:, 3] / size
    kp = r[:, 4:].reshape(-1, 6, 2) / size + a[:, None, :]
    rows = np.concatenate(
        [s[:, None], (cx - w / 2)[:, None], (cy - h / 2)[:, None], (cx + w / 2)[:, None], (cy + h / 2)[:, None],
         kp.reshape(-1, 12)],
        axis=1,
    )  # fmt: skip
    return [row for row in rows if row[0] >= min_score]


def _iou(a: np.ndarray, b: np.ndarray) -> float:
    ix = max(0.0, min(a[3], b[3]) - max(a[1], b[1]))
    iy = max(0.0, min(a[4], b[4]) - max(a[2], b[2]))
    inter = ix * iy
    union = (a[3] - a[1]) * (a[4] - a[2]) + (b[3] - b[1]) * (b[4] - b[2]) - inter
    return inter / union if union > 0 else 0.0


def weighted_nms(rows: list[np.ndarray], threshold: float = 0.3) -> list[np.ndarray]:
    """MediaPipe WEIGHTED NMS: each kept detection's geometry is the score-weighted mean of its cluster."""
    remaining = sorted(rows, key=lambda r: -r[0])
    out = []
    while remaining:
        top = remaining[0]
        cluster = [r for r in remaining if _iou(top, r) > threshold]
        rest = [r for r in remaining if _iou(top, r) <= threshold]
        weights = np.array([r[0] for r in cluster])
        geom = np.sum([r[1:] * r[0] for r in cluster], axis=0) / weights.sum()
        out.append(np.concatenate([[top[0]], geom]))
        remaining = rest
    return out


def to_image(row: np.ndarray, width: int, height: int, pad: tuple[float, float, float, float]) -> Detection:
    """Remove letterbox padding and scale to image pixels."""
    left, top, right, bottom = pad
    sx, sy = 1.0 - left - right, 1.0 - top - bottom

    def px(x: float, y: float) -> tuple[float, float]:
        return (x - left) / sx * width, (y - top) / sy * height

    x0, y0 = px(row[1], row[2])
    x1, y1 = px(row[3], row[4])
    kps = [px(row[5 + 2 * k], row[6 + 2 * k]) for k in range(6)]
    return Detection((x0, y0, x1 - x0, y1 - y0), float(row[0]), kps)


def letterbox_roi(width: int, height: int, variant: str) -> tuple[RotatedRect, tuple[float, float, float, float]]:
    size = CONFIGS[variant][0]
    return pad_roi(whole_image(width, height), size, size)


def landmark_roi(det: Detection) -> RotatedRect:
    """Detection -> landmark ROI (DetectionsToRects + RectTransformation x1.5, MediaPipe face landmarker)."""
    x, y, w, h = det.box
    (x0, y0), (x1, y1) = det.keypoints[0], det.keypoints[1]
    rotation = 0.0 - math.atan2(-(y1 - y0), x1 - x0)
    rotation = (rotation + math.pi) % (2 * math.pi) - math.pi
    return RotatedRect(x + w / 2, y + h / 2, w * 1.5, h * 1.5, rotation)
