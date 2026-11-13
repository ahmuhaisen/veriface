"""L3: shipped ONNX vs the original runtime, per output, on test images plus fixed random tensors.

Reference runtime: ``tf.lite.Interpreter`` (TensorFlow-CPU) for the converted MediaPipe models; for AuraFace
the reference is the unmodified upstream ONNX on ONNX Runtime (we only rename its I/O).
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import cv2
import numpy as np
import onnxruntime as ort

from facelab.paths import IMAGES_DIR
from facelab.specs import ModelSpec

LANDMARK_SUBSET = [
    0, 1, 4, 5, 6, 7, 8, 10, 13, 14, 17, 21, 33, 37, 39, 40, 46, 52, 53, 54, 55, 58, 61, 63, 65, 66, 67, 70,
    78, 80, 81, 82, 84, 87, 88, 91, 93, 95, 103, 105, 107, 109, 127, 132, 133, 136, 144, 145, 146, 148, 149,
    150, 152, 153, 154, 155, 157, 158, 159, 160, 161, 162, 163, 168, 172, 173, 176, 178, 181, 185, 191, 195,
    197, 234, 246, 249, 251, 263, 267, 269, 270, 276, 282, 283, 284, 285, 288, 291, 293, 295, 296, 297, 300,
    308, 310, 311, 312, 314, 317, 318, 321, 323, 324, 332, 334, 336, 338, 356, 361, 362, 365, 373, 374, 375,
    377, 378, 379, 380, 381, 382, 384, 385, 386, 387, 388, 389, 390, 397, 398, 400, 402, 405, 409, 415, 454,
    466, 468, 469, 470, 471, 472, 473, 474, 475, 476, 477,
]  # fmt: skip
N_RANDOM = 3
# Relative part of the allclose test. Raw detector outputs are unbounded (anchors in the letterbox padding
# reach |logit| ~ 1e4), where float32 accumulation alone is ~1e-2 absolute: TFLite's own XNNPACK and builtin
# reference kernels disagree by up to ~1.6e-5 relative on the short-range detector (2026-10-01, 22 test
# images). 5e-5 is ~3x that spread. A pure absolute tolerance fails on rounding, not on conversion.
RTOL = 5e-5


@dataclass
class FidelityResult:
    model_id: str
    n_images: int
    n_random: int
    max_abs: dict[str, float]
    budget_used: dict[str, float]  # max over elements of |a-b| / (atol + RTOL*|ref|); must be <= 1
    tolerance: dict[str, float]  # atol per output
    reference: str
    noise_floor: dict[str, float] | None = None  # same metric as max_abs between two reference runtimes

    @property
    def ok(self) -> bool:
        return all(v <= 1.0 for v in self.budget_used.values())

    def summary(self) -> str:
        parts = ", ".join(
            f"{k} {v:.2e} (atol {self.tolerance[k]:.0e}, rtol {RTOL:.0e}: {self.budget_used[k]:.2f} of budget)"
            for k, v in self.max_abs.items()
        )
        out = f"max abs diff vs {self.reference}: {parts}"
        if self.noise_floor:
            nf = ", ".join(f"{k} {v:.2e}" for k, v in self.noise_floor.items())
            out += f"; reference noise floor (TFLite XNNPACK vs TFLite builtin reference kernels): {nf}"
        return f"{out}; on {self.n_images} test images + {self.n_random} seeded random tensors"


def budget_used(ref: np.ndarray, got: np.ndarray, atol: float, rtol: float = RTOL) -> float:
    """Worst fraction of the allclose budget ``atol + rtol*|ref|`` used by ``|ref - got|`` (<= 1 passes)."""
    diff = np.abs(ref - got)
    allowed = atol + rtol * np.abs(ref)
    exceeded_zero_budget = (allowed == 0) & (diff > 0)
    if exceeded_zero_budget.any():
        return float("inf")
    ratio = np.divide(diff, allowed, out=np.zeros_like(diff), where=allowed > 0)
    return float(ratio.max()) if ratio.size else 0.0


def test_images() -> list[np.ndarray]:
    """RGB uint8 images from testdata/images (empty until L4 lands)."""
    out = []
    if IMAGES_DIR.exists():
        for p in sorted(IMAGES_DIR.iterdir()):
            if p.suffix.lower() in {".jpg", ".jpeg", ".png"}:
                bgr = cv2.imread(str(p), cv2.IMREAD_COLOR)
                if bgr is not None:
                    out.append(cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB))
    return out


def letterbox(rgb: np.ndarray, size: int) -> np.ndarray:
    h, w = rgb.shape[:2]
    s = size / max(h, w)
    nw, nh = round(w * s), round(h * s)
    canvas = np.zeros((size, size, 3), np.uint8)
    x0, y0 = (size - nw) // 2, (size - nh) // 2
    canvas[y0 : y0 + nh, x0 : x0 + nw] = cv2.resize(rgb, (nw, nh), interpolation=cv2.INTER_LINEAR)
    return canvas


def center_square(rgb: np.ndarray, size: int) -> np.ndarray:
    h, w = rgb.shape[:2]
    s = min(h, w)
    y0, x0 = (h - s) // 2, (w - s) // 3  # portraits: bias towards the face
    return cv2.resize(rgb[y0 : y0 + s, x0 : x0 + s], (size, size), interpolation=cv2.INTER_AREA)


def _norm(img: np.ndarray, value_range: str) -> np.ndarray:
    x = img.astype(np.float32)
    x = x / 255.0 if value_range.startswith("x/255") else (x - 127.5) / 127.5
    return x[None]


class _Tflite:
    def __init__(self, path: Path, reference_kernels: bool = False):
        import tensorflow as tf

        resolver = (
            tf.lite.experimental.OpResolverType.BUILTIN_WITHOUT_DEFAULT_DELEGATES
            if reference_kernels
            else tf.lite.experimental.OpResolverType.AUTO
        )
        self.it = tf.lite.Interpreter(model_path=str(path), experimental_op_resolver_type=resolver)
        self.it.allocate_tensors()
        self.inp = self.it.get_input_details()[0]["index"]
        self.outs = [d["index"] for d in self.it.get_output_details()]

    def __call__(self, x: np.ndarray) -> list[np.ndarray]:
        self.it.set_tensor(self.inp, x)
        self.it.invoke()
        return [self.it.get_tensor(i).copy() for i in self.outs]


class _Onnx:
    def __init__(self, path: Path):
        so = ort.SessionOptions()
        so.intra_op_num_threads = 1
        self.s = ort.InferenceSession(str(path), so, providers=["CPUExecutionProvider"])
        self.inp = self.s.get_inputs()[0].name

    def __call__(self, x: np.ndarray) -> list[np.ndarray]:
        return self.s.run(None, {self.inp: x})


def _inputs(spec: ModelSpec, images: list[np.ndarray], landmarker_onnx: Path | None) -> list[np.ndarray]:
    rng = np.random.default_rng(20261001)
    t = spec.inputs[0]
    xs: list[np.ndarray] = []
    if spec.role == "face-detector":
        size = t.shape[1]
        xs += [_norm(letterbox(im, size), t.value_range) for im in images]
        xs += [rng.uniform(-1, 1, (1, size, size, 3)).astype(np.float32) for _ in range(N_RANDOM)]
    elif spec.role in {"face-landmarker", "segmenter"}:
        size = t.shape[1]
        xs += [_norm(center_square(im, size), t.value_range) for im in images]
        lo = 0.0 if t.value_range.startswith("x/255") else -1.0
        xs += [rng.uniform(lo, 1, (1, size, size, 3)).astype(np.float32) for _ in range(N_RANDOM)]
    elif spec.role == "face-blendshapes":
        if landmarker_onnx is not None:
            lm = _Onnx(landmarker_onnx)
            for im in images:
                pts = lm(_norm(center_square(im, 256), "x/255"))[0].reshape(478, 3)[:, :2]
                xs.append(pts[LANDMARK_SUBSET][None].astype(np.float32))
        xs += [rng.uniform(0, 512, (1, 146, 2)).astype(np.float32) for _ in range(N_RANDOM)]
    elif spec.role == "face-embedder":
        for im in images:
            x = center_square(im, 112).astype(np.float32)
            xs.append(((x - 127.5) / 127.5).transpose(2, 0, 1)[None])
        xs += [rng.uniform(-1, 1, (1, 3, 112, 112)).astype(np.float32) for _ in range(N_RANDOM)]
    return xs


def check(spec: ModelSpec, upstream: Path, shipped: Path, landmarker_onnx: Path | None = None) -> FidelityResult:
    images = test_images()
    xs = _inputs(spec, images, landmarker_onnx)
    if spec.converted:
        ref, reference = _Tflite(upstream), "tf.lite.Interpreter (TensorFlow-CPU) on the upstream TFLite"
    else:
        ref, reference = _Onnx(upstream), "the unmodified upstream ONNX on ONNX Runtime"
    alt = _Tflite(upstream, reference_kernels=True) if spec.converted else None
    got = _Onnx(shipped)
    names = [o.name for o in spec.outputs]
    worst = dict.fromkeys(names, 0.0)
    budget = dict.fromkeys(names, 0.0)
    floor = dict.fromkeys(names, 0.0)
    for x in xs:
        refs = ref(x)
        for name, a, b in zip(names, refs, got(x), strict=True):
            a = a.reshape(b.shape)
            diff = np.abs(a - b)
            worst[name] = max(worst[name], float(diff.max()))
            budget[name] = max(budget[name], budget_used(a, b, spec.tolerances[name]))
        if alt is not None:
            for name, a, c in zip(names, refs, alt(x), strict=True):
                floor[name] = max(floor[name], float(np.max(np.abs(a - c))))
    n_img = len(images)
    return FidelityResult(
        spec.id,
        n_img,
        len(xs) - n_img,
        worst,
        budget,
        dict(spec.tolerances),
        reference,
        floor if alt is not None else None,
    )
