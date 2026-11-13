"""Experiment: fold/cancel Transposes in the shipped ONNX models; check exactness, latency and reproducibility.

    lab/.venv/Scripts/python.exe -m facelab.experiments.segmenter_transposes [model-id ...]

Writes candidates to lab/work/transpose-fold/ only; never touches models/.
"""

from __future__ import annotations

import hashlib
import sys
import time
from pathlib import Path

import numpy as np
import onnx
import onnxruntime as ort

from facelab.paths import MODELS_DIR, WORK_DIR
from facelab.specs import MODELS_BY_ID
from facelab.transpose_fold import fold_transposes

OUT = WORK_DIR / "transpose-fold"


def _session(path: Path) -> ort.InferenceSession:
    so = ort.SessionOptions()
    so.intra_op_num_threads = 1
    return ort.InferenceSession(str(path), so, providers=["CPUExecutionProvider"])


def _latency(paths: dict[str, Path], x: dict[str, np.ndarray], rounds: int = 6, runs: int = 8) -> dict[str, float]:
    sess = {k: _session(p) for k, p in paths.items()}
    for s in sess.values():
        for _ in range(3):
            s.run(None, x)
    t: dict[str, list[float]] = {k: [] for k in sess}
    for _ in range(rounds):
        for k, s in sess.items():
            for _ in range(runs):
                t0 = time.perf_counter()
                s.run(None, x)
                t[k].append(time.perf_counter() - t0)
    return {k: round(float(np.median(v) * 1000), 1) for k, v in t.items()}


def run(model_id: str) -> dict:
    OUT.mkdir(parents=True, exist_ok=True)
    src = MODELS_DIR / f"{model_id}.onnx"
    hashes = []
    for i in range(2):  # reproducibility: build twice
        m = onnx.load(str(src))
        stats = fold_transposes(m)
        onnx.checker.check_model(m)
        dst = OUT / f"{model_id}.folded.{i}.onnx"
        onnx.save(m, str(dst))
        hashes.append(hashlib.sha256(dst.read_bytes()).hexdigest())
    spec = MODELS_BY_ID[model_id]
    t = spec.inputs[0]
    shape = [1 if not isinstance(d, int) else d for d in t.shape]
    rng = np.random.default_rng(0)
    lo = 0.0 if (t.value_range or "").startswith("x/255") else -1.0
    xs = [rng.uniform(lo, 1, shape).astype(np.float32) for _ in range(4)]
    a, b = _session(src), _session(OUT / f"{model_id}.folded.0.onnx")
    diff = max(
        float(np.max(np.abs(p - q)))
        for x in xs
        for p, q in zip(a.run(None, {t.name: x}), b.run(None, {t.name: x}), strict=True)
    )
    n_before = sum(1 for n in onnx.load(str(src)).graph.node if n.op_type == "Transpose")
    n_after = sum(1 for n in onnx.load(str(OUT / f"{model_id}.folded.0.onnx")).graph.node if n.op_type == "Transpose")
    lat = _latency({"shipped": src, "folded": OUT / f"{model_id}.folded.0.onnx"}, {t.name: xs[0]})
    r = {
        "model": model_id,
        "transposes": f"{n_before} -> {n_after}",
        **stats,
        "maxAbsDiffVsShipped": diff,
        "reproducible": hashes[0] == hashes[1],
        "sha256": hashes[0],
        "latencyMs1Thread": lat,
    }
    print(r)
    return r


if __name__ == "__main__":
    for mid in sys.argv[1:] or ["mp-selfie-multiclass"]:
        run(mid)
