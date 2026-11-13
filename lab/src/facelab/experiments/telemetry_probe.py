"""Does a MediaPipe build phone home? Run Tasks inference and record every remote endpoint the process contacts.

Standalone on purpose (only mediapipe + numpy + opencv), so it runs in any candidate venv:

    <venv>/Scripts/python.exe lab/src/facelab/experiments/telemetry_probe.py [seconds]

Polls `netstat -ano` for this PID (TCP and UDP) every 0.5 s from BEFORE mediapipe is imported. Uses only the
pinned upstream models in models/upstream/ and a public-domain fixture. Exit code 0 = no remote endpoint seen,
3 = contacted something (printed).

Finding (2026-10-01): mediapipe 1.0.1 embeds Google's Clearcut logger (https://play.googleapis.com/log) in
libmediapipe.dll and opened TCP 443/80 to Google ~65 s into local-only inference.
"""

from __future__ import annotations

import os
import subprocess
import sys
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]
UP = REPO / "models" / "upstream"
IMAGE = REPO / "testdata" / "images" / "nasa-glover.jpg"
LOCAL = ("0.0.0.0", "[::]", "*:", "127.0.0.1", "[::1]")


def main() -> int:
    duration = float(sys.argv[1]) if len(sys.argv) > 1 else 150.0
    pid = str(os.getpid())
    seen: dict[str, float] = {}
    t_start = time.time()
    stop = threading.Event()

    def poll() -> None:
        while not stop.is_set():
            out = subprocess.run(["netstat", "-ano"], capture_output=True, text=True).stdout
            for line in out.splitlines():
                parts = line.split()
                if len(parts) >= 4 and parts[-1] == pid and parts[0] in ("TCP", "UDP"):
                    remote = parts[2]
                    if remote not in seen and not remote.startswith(LOCAL):
                        seen[remote] = time.time() - t_start
                        print(f"  t={seen[remote]:6.1f}s {parts[0]} -> {remote}", flush=True)
            stop.wait(0.5)

    th = threading.Thread(target=poll, daemon=True)
    th.start()

    import cv2
    import mediapipe as mp
    import numpy as np
    from mediapipe.tasks.python import BaseOptions, vision

    det = vision.FaceDetector.create_from_options(
        vision.FaceDetectorOptions(base_options=BaseOptions(model_asset_path=str(UP / "blaze_face_short_range.tflite")))
    )
    lmk = vision.FaceLandmarker.create_from_options(
        vision.FaceLandmarkerOptions(
            base_options=BaseOptions(model_asset_path=str(UP / "face_landmarker.task")),
            output_face_blendshapes=True,
            output_facial_transformation_matrixes=True,
        )
    )
    seg = vision.ImageSegmenter.create_from_options(
        vision.ImageSegmenterOptions(
            base_options=BaseOptions(model_asset_path=str(UP / "selfie_multiclass_256x256.tflite")),
            output_confidence_masks=True,
        )
    )
    rgb = np.ascontiguousarray(cv2.cvtColor(cv2.imread(str(IMAGE)), cv2.COLOR_BGR2RGB))
    image = mp.Image(image_format=mp.ImageFormat.SRGB, data=rgb)
    n = 0
    while time.time() - t_start < duration:
        det.detect(image)
        lmk.detect(image)
        seg.segment(image)
        n += 1
    stop.set()
    th.join(2)
    print(f"mediapipe {mp.__version__}: {n} rounds over {duration:.0f}s; remote endpoints contacted: {len(seen)}")
    return 3 if seen else 0


if __name__ == "__main__":
    raise SystemExit(main())
