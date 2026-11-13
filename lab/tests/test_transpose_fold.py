"""transpose_fold: exact rewrites only (needs onnx + onnxruntime: lab/.venv)."""

from __future__ import annotations

import numpy as np
import pytest

onnx = pytest.importorskip("onnx", reason="needs lab/.venv")
ort = pytest.importorskip("onnxruntime")

from onnx import TensorProto, helper, numpy_helper  # noqa: E402

from facelab.transpose_fold import fold_transposes  # noqa: E402


def _model() -> onnx.ModelProto:
    w = numpy_helper.from_array(np.arange(24, dtype=np.float32).reshape(2, 3, 4), "w")
    nodes = [
        helper.make_node("Transpose", ["x"], ["a"], perm=[0, 2, 1]),  # pair: (0,2,1) o (0,2,1) = identity
        helper.make_node("Transpose", ["a"], ["b"], perm=[0, 2, 1]),
        helper.make_node("Transpose", ["w"], ["wt"], perm=[0, 2, 1]),  # constant -> folded
        helper.make_node("MatMul", ["b", "wt"], ["y"]),
    ]
    g = helper.make_graph(
        nodes,
        "g",
        [helper.make_tensor_value_info("x", TensorProto.FLOAT, [2, 5, 4])],
        [helper.make_tensor_value_info("y", TensorProto.FLOAT, [2, 5, 3])],
        initializer=[w],
    )
    return helper.make_model(g, opset_imports=[helper.make_opsetid("", 17)], ir_version=10)  # ORT 1.28: IR <= 13


def _run(m: onnx.ModelProto, x: np.ndarray) -> np.ndarray:
    return ort.InferenceSession(m.SerializeToString(), providers=["CPUExecutionProvider"]).run(None, {"x": x})[0]


def test_fold_is_exact_and_removes_all_transposes():
    x = np.random.default_rng(0).normal(size=(2, 5, 4)).astype(np.float32)
    before = _model()
    after = _model()
    stats = fold_transposes(after)
    onnx.checker.check_model(after)
    assert stats == {"constantFolded": 1, "pairsCancelled": 1}
    assert [n.op_type for n in after.graph.node] == ["MatMul"]
    assert np.array_equal(_run(before, x), _run(after, x))
    again = _model()
    fold_transposes(again)
    assert again.SerializeToString() == after.SerializeToString()  # reproducible
