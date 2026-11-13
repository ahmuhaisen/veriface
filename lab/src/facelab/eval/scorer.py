"""Lab reference scorer: image -> primary face -> raw pose + aligned AuraFace embedding.

Uses the same path as the goldens (MediaPipe Tasks detection and landmarks, ArcFace alignment, AuraFace on
ONNX Runtime). A sample with no face is a failure to acquire (FTA) and is excluded from comparisons.
The service can be scored instead by supplying a scores file (run.py), which is what calibration of the
shipped pipeline ultimately needs.
"""

from __future__ import annotations

from dataclasses import dataclass

import cv2
import numpy as np

from facelab import align, golden, pose
from facelab.eval.dataset import Sample


@dataclass
class Scored:
    sample: str
    face_found: bool
    embedding: np.ndarray | None = None
    pose: dict | None = None
    faces: int = 0


def score_sample(s: Sample) -> Scored:
    bgr = cv2.imread(str(s.path))
    if bgr is None:
        raise FileNotFoundError(s.path)
    rgb = cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB)
    full = golden._detections(rgb, "full")
    faces = golden._landmarker_faces(rgb, full, crops_only=s.kind == "document")  # same routes as the goldens
    if not faces:
        return Scored(s.id, False)
    _, _, pts, _, _ = faces[0]
    crop, _ = align.aligned_crop(rgb, pts[list(align.LANDMARKS5), :2])
    _, emb = align.embed(crop)
    fit = pose.fit_weak_perspective(pts)
    return Scored(s.id, True, emb, {k: fit[k] for k in ("yaw", "pitch", "roll")}, len(faces))
