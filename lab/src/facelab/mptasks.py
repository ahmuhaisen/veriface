"""Official MediaPipe Tasks runners (the reference implementation for goldens and test-data crops).

PRIVACY: only telemetry-free MediaPipe builds may run here. mediapipe 0.10.35 and 1.0.x embed Google's Clearcut
logger (https://play.googleapis.com/log) in their native library and contact Google about 60 s into local
inference; 0.10.21 and 0.10.14 have no Clearcut code and contacted nothing in a 180 s probe
(lab/src/facelab/experiments/telemetry_probe.py, 2026-10-01). Importing this module with any other version fails.
Run it from lab/.venv-mp (see lab/README.md).
"""

from __future__ import annotations

from functools import cache

import mediapipe as mp
import numpy as np
from mediapipe.tasks.python import BaseOptions, vision

from facelab.specs import MODELS_BY_ID
from facelab.upstream import SOURCES, upstream_file

TELEMETRY_FREE_MEDIAPIPE = frozenset({"0.10.21"})  # probe-verified; add a version only after re-running the probe

if mp.__version__ not in TELEMETRY_FREE_MEDIAPIPE:
    raise ImportError(
        f"mediapipe {mp.__version__} is not on the telemetry-free list {sorted(TELEMETRY_FREE_MEDIAPIPE)}. "
        "Use lab/.venv-mp (lab/README.md). Newer builds send usage telemetry to Google."
    )


def _image(rgb: np.ndarray) -> mp.Image:
    return mp.Image(image_format=mp.ImageFormat.SRGB, data=np.ascontiguousarray(rgb))


@cache
def face_detector(variant: str = "full", min_confidence: float = 0.5) -> vision.FaceDetector:
    path = (
        SOURCES["face_detection_full_range_with_metadata"].path
        if variant == "full"
        else upstream_file(MODELS_BY_ID["mp-face-detector-short"])
    )
    opts = vision.FaceDetectorOptions(
        base_options=BaseOptions(model_asset_path=str(path)),
        running_mode=vision.RunningMode.IMAGE,
        min_detection_confidence=min_confidence,
        min_suppression_threshold=0.3,
    )
    return vision.FaceDetector.create_from_options(opts)


@cache
def face_landmarker(num_faces: int = 1) -> vision.FaceLandmarker:
    opts = vision.FaceLandmarkerOptions(
        base_options=BaseOptions(model_asset_path=str(SOURCES["face_landmarker_task"].path)),
        running_mode=vision.RunningMode.IMAGE,
        num_faces=num_faces,
        output_face_blendshapes=True,
        output_facial_transformation_matrixes=True,
    )
    return vision.FaceLandmarker.create_from_options(opts)


@cache
def segmenter() -> vision.ImageSegmenter:
    opts = vision.ImageSegmenterOptions(
        base_options=BaseOptions(model_asset_path=str(SOURCES["selfie_multiclass"].path)),
        running_mode=vision.RunningMode.IMAGE,
        output_confidence_masks=True,
        output_category_mask=True,
    )
    return vision.ImageSegmenter.create_from_options(opts)


def detect_faces(rgb: np.ndarray, variant: str = "short") -> list:
    """Short-range only on the pinned 0.10.21 (its Tasks FaceDetector builds short-range anchors only)."""
    if variant != "short":
        raise ValueError("full-range: use facelab.ref.pipeline.detect (see golden.DETECTION_SOURCES)")
    return face_detector(variant).detect(_image(rgb)).detections


def landmarks(rgb: np.ndarray, num_faces: int = 1):
    return face_landmarker(num_faces).detect(_image(rgb))


def segment(rgb: np.ndarray):
    return segmenter().segment(_image(rgb))
