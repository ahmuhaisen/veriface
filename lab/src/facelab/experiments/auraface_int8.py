"""L6: AuraFace INT8 vs FP32: size, CPU latency, embedding drift.

Faces are the aligned 112x112 crops of every face in testdata/golden/pipeline (PD + synthetic only). They are
split in two: static quantization is calibrated on one half, drift is reported on the other half (held out) and
on all faces. Nothing produced here ships; quantized models stay in lab/work/.
"""

from __future__ import annotations

import json
import time
from pathlib import Path

import cv2
import numpy as np
import onnx
import onnxruntime as ort
from onnx import version_converter
from onnxruntime.quantization import (
    CalibrationDataReader,
    CalibrationMethod,
    QuantFormat,
    QuantType,
    quantize_dynamic,
    quantize_static,
)
from onnxruntime.quantization.shape_inference import quant_pre_process

from facelab import align
from facelab.paths import GOLDEN_DIR, MODELS_DIR, TESTDATA_DIR, WORK_DIR

OUT = WORK_DIR / "auraface-int8"


def _faces() -> list[tuple[str, np.ndarray]]:
    out = []
    for path in sorted((GOLDEN_DIR / "pipeline").glob("*.json")):
        g = json.loads(path.read_text(encoding="utf-8"))
        if not g["faces"]:
            continue
        rgb = cv2.cvtColor(cv2.imread(str(TESTDATA_DIR / g["image"]["file"])), cv2.COLOR_BGR2RGB)
        for k, f in enumerate(g["faces"]):
            crop, _ = align.aligned_crop(rgb, np.array(f["landmarks5"]))
            x = ((crop.astype(np.float32) - 127.5) / 127.5).transpose(2, 0, 1)[None]
            out.append((f"{g['image']['id']}#{k}", x))
    return out


class _Reader(CalibrationDataReader):
    def __init__(self, xs: list[np.ndarray]):
        self._it = iter([{"input": x} for x in xs])

    def get_next(self):
        return next(self._it, None)


def _session(path: Path, threads: int) -> ort.InferenceSession:
    so = ort.SessionOptions()
    so.intra_op_num_threads = threads
    return ort.InferenceSession(str(path), so, providers=["CPUExecutionProvider"])


def _embed(sess: ort.InferenceSession, xs: list[np.ndarray]) -> np.ndarray:
    e = np.concatenate([sess.run(None, {"input": x})[0] for x in xs])
    return e / np.linalg.norm(e, axis=1, keepdims=True)


def _latency_ms(
    paths: dict[str, Path], threads: int, x: np.ndarray, rounds: int = 6, runs: int = 8
) -> dict[str, float]:
    """Median latency per variant, measured INTERLEAVED (round-robin) so machine-load drift hits all equally."""
    sessions = {k: _session(p, threads) for k, p in paths.items()}
    for s in sessions.values():
        for _ in range(3):
            s.run(None, {"input": x})
    times: dict[str, list[float]] = {k: [] for k in sessions}
    for _ in range(rounds):
        for k, s in sessions.items():
            for _ in range(runs):
                t = time.perf_counter()
                s.run(None, {"input": x})
                times[k].append(time.perf_counter() - t)
    return {k: round(float(np.median(v) * 1000), 1) for k, v in times.items()}


# Static QDQ variants: (calibration method, extra options, nodes kept in float).
STATIC_VARIANTS = {
    "int8-qdq-minmax": (CalibrationMethod.MinMax, {}, []),
    "int8-qdq-percentile": (CalibrationMethod.Percentile, {"CalibPercentile": 99.999}, []),
    "int8-qdq-entropy": (CalibrationMethod.Entropy, {}, []),
    "int8-qdq-minmax-fp-ends": (CalibrationMethod.MinMax, {}, ["/conv1/Conv", "/fc/Gemm"]),
    "int8-qdq-percentile-fp-ends": (
        CalibrationMethod.Percentile,
        {"CalibPercentile": 99.999},
        ["/conv1/Conv", "/fc/Gemm"],
    ),
}


def run() -> dict:
    OUT.mkdir(parents=True, exist_ok=True)
    fp32 = MODELS_DIR / "auraface-v1.onnx"
    # The upstream graph is opset 11; per-channel QDQ (DequantizeLinear with axis) needs opset >= 13.
    upgraded = OUT / "auraface-v1.opset17.onnx"
    if not upgraded.exists():
        onnx.save(version_converter.convert_version(onnx.load(str(fp32)), 17), str(upgraded))
    pre = OUT / "auraface-v1.pre.onnx"
    if not pre.exists():
        quant_pre_process(str(upgraded), str(pre), skip_symbolic_shape=True)
    faces = _faces()
    names = [n for n, _ in faces]
    xs = [x for _, x in faces]
    calib_idx = list(range(0, len(xs), 2))
    held_idx = list(range(1, len(xs), 2))

    variants = {"fp32": fp32, "fp32-opset17": upgraded}
    dyn = OUT / "auraface-v1.int8-dynamic.onnx"
    if not dyn.exists():
        quantize_dynamic(str(pre), str(dyn), weight_type=QuantType.QInt8, per_channel=True)
    variants["int8-dynamic"] = dyn
    # Calibration set: the calibration half, plus horizontal flips (still only PD/synthetic fixtures).
    calib = [xs[i] for i in calib_idx] + [np.ascontiguousarray(xs[i][..., ::-1]) for i in calib_idx]
    for name, (method, extra, keep_float) in STATIC_VARIANTS.items():
        path = OUT / f"auraface-v1.{name}.onnx"
        if not path.exists():
            quantize_static(
                str(pre),
                str(path),
                _Reader(calib),
                quant_format=QuantFormat.QDQ,
                per_channel=True,
                activation_type=QuantType.QUInt8,
                weight_type=QuantType.QInt8,
                calibrate_method=method,
                nodes_to_exclude=keep_float,
                extra_options=extra,
            )
        variants[name] = path

    ref = _embed(_session(fp32, 1), xs)
    timed = {k: p for k, p in variants.items() if k != "int8-dynamic"}  # dynamic is ~20x slower; timed once
    lat = {1: _latency_ms(timed, 1, xs[0]), 4: _latency_ms(timed, 4, xs[0])}
    for t in (1, 4):
        lat[t]["int8-dynamic"] = _latency_ms({"d": variants["int8-dynamic"]}, t, xs[0], rounds=1, runs=5)["d"]
    report = {"faces": len(xs), "calibration": len(calib), "heldOut": len(held_idx), "variants": {}}
    for name, path in variants.items():
        emb = _embed(_session(path, 1), xs)
        cos = np.sum(emb * ref, axis=1)
        # Effect on the decision space: genuine/impostor scores among the PD people.
        sim = emb @ emb.T
        report["variants"][name] = {
            "file": path.name,
            "mb": round(path.stat().st_size / 1e6, 1),
            "latencyMs1Thread": lat[1][name],
            "latencyMs4Threads": lat[4][name],
            "cosToFp32": {
                "all": {"min": round(float(cos.min()), 5), "mean": round(float(cos.mean()), 5)},
                "heldOut": {"min": round(float(cos[held_idx].min()), 5), "mean": round(float(cos[held_idx].mean()), 5)},
                "worst": names[int(cos.argmin())],
            },
            "maxScoreShiftVsFp32": round(float(np.abs(sim - ref @ ref.T).max()), 5),
        }
        print(name, json.dumps(report["variants"][name]))
    # Acceptance (lead, 2026-10-01): cosine to FP32 >= 0.995 on every face AND faster than FP32 at the same
    # thread count (both 1 and 4 threads).
    base = report["variants"]["fp32"]
    for name, v in report["variants"].items():
        if name.startswith("int8"):
            v["accepted"] = (
                v["cosToFp32"]["all"]["min"] >= 0.995
                and v["latencyMs1Thread"] < base["latencyMs1Thread"]
                and v["latencyMs4Threads"] < base["latencyMs4Threads"]
            )
    report["recommendation"] = next(
        (f"ship {n}" for n, v in report["variants"].items() if v.get("accepted")),
        "ship FP32 (no INT8 variant accepted)",
    )
    print(report["recommendation"])
    (OUT / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return report
