"""Unit tests for the model pipeline. No network; tests needing built artifacts skip when they are absent."""

from __future__ import annotations

import json
import re

import jsonschema
import numpy as np
import pytest

onnx = pytest.importorskip("onnx", reason="needs lab/.venv (model conversion tools)")

from onnx import TensorProto, helper  # noqa: E402

from facelab import convert, fidelity, specs, upstream  # noqa: E402
from facelab.paths import MANIFEST_PATH, MANIFEST_SCHEMA_PATH, MODELS_DIR  # noqa: E402

REQUIRED_IDS = {
    "mp-face-detector-short",
    "mp-face-detector-full",
    "mp-face-landmarker",
    "mp-face-blendshapes",
    "mp-selfie-multiclass",
    "auraface-v1",
}


def test_model_ids_are_the_agreed_set():
    assert {m.id for m in specs.MODELS} == REQUIRED_IDS


def test_every_source_is_pinned():
    for src in upstream.SOURCES.values():
        assert re.fullmatch(r"[0-9a-f]{64}", src.sha256), src.key
        assert src.url.startswith("https://"), src.key


def test_auraface_source_is_only_the_recognition_model():
    src = upstream.SOURCES["auraface"]
    assert src.url.endswith("/glintr100.onnx")
    assert src.url.split("/")[-1] not in upstream.FORBIDDEN_NAMES


def test_forbidden_insightface_files_are_detected(tmp_path):
    upstream.check_forbidden(tmp_path)  # clean dir passes
    (tmp_path / "scrfd_10g_bnkps.onnx").write_bytes(b"x")
    with pytest.raises(upstream.IntegrityError):
        upstream.check_forbidden(tmp_path)


def test_fetch_rejects_hash_mismatch(tmp_path, monkeypatch):
    monkeypatch.setattr(upstream, "UPSTREAM_DIR", tmp_path)
    (tmp_path / "f.bin").write_bytes(b"tampered")
    src = upstream.Source("k", "f.bin", "https://example.invalid/f", "0" * 64, "n", "v", "https://example.invalid")
    monkeypatch.setattr(upstream.Source, "path", property(lambda s: tmp_path / s.filename))
    with pytest.raises(upstream.IntegrityError, match="mismatch"):
        upstream.fetch(src)


def test_specs_tolerances_cover_every_output():
    for m in specs.MODELS:
        assert set(m.tolerances) == {t.name for t in m.outputs}, m.id


def test_blendshape_subset_matches_spec_notes():
    assert len(fidelity.LANDMARK_SUBSET) == 146 == len(set(fidelity.LANDMARK_SUBSET))
    notes = specs.MODELS_BY_ID["mp-face-blendshapes"].notes
    listed = re.search(r"in this order\): ([0-9,]+)\.", notes).group(1)
    assert [int(v) for v in listed.split(",")] == fidelity.LANDMARK_SUBSET


def _tiny_model(prefix: str) -> onnx.ModelProto:
    w = helper.make_tensor(f"{prefix}_w", TensorProto.FLOAT, [1], [2.0])
    b = helper.make_tensor(f"{prefix}_b", TensorProto.FLOAT, [1], [1.0])
    nodes = [
        helper.make_node("Mul", ["x", f"{prefix}_w"], [f"{prefix}_m"], name=f"{prefix}_mul"),
        helper.make_node("Add", [f"{prefix}_m", f"{prefix}_b"], ["y"], name=f"{prefix}_add"),
    ]
    graph = helper.make_graph(
        nodes,
        "g",
        [helper.make_tensor_value_info("x", TensorProto.FLOAT, ["unk__1"])],
        [helper.make_tensor_value_info("y", TensorProto.FLOAT, ["unk__2"])],
        initializer=[b, w],  # deliberately out of use order
    )
    return helper.make_model(graph)


def test_canonicalize_is_independent_of_generated_names():
    a, b = _tiny_model("conv__48"), _tiny_model("conv__533")
    convert._canonicalize(a)
    convert._canonicalize(b)
    assert a.SerializeToString() == b.SerializeToString()
    assert [t.name for t in a.graph.initializer] == ["w0", "w2"]  # first-use order
    onnx.checker.check_model(a)


def test_rename_and_batch():
    m = _tiny_model("p")
    convert._rename(m, {"x": "input", "y": "out"})
    convert._set_batch(m, 1)
    assert [i.name for i in m.graph.input] == ["input"]
    assert m.graph.node[1].output[0] == "out"
    assert m.graph.input[0].type.tensor_type.shape.dim[0].dim_value == 1


def test_fidelity_result_flags_excess():
    r = fidelity.FidelityResult("m", 0, 3, {"a": 2e-3}, {"a": 2.0}, {"a": 1e-3}, "ref")
    assert not r.ok
    assert fidelity.FidelityResult("m", 0, 3, {"a": 1e-4}, {"a": 0.1}, {"a": 1e-3}, "ref").ok


def test_budget_used_is_allclose():
    ref = np.array([0.0, 1.0, 1e4], np.float32)
    # 1e-2 off on a 1e4-magnitude value is within rtol (budget 1e-3 + 0.5); same miss at 1.0 is not.
    assert fidelity.budget_used(ref, ref + np.array([0, 0, 1e-2], np.float32), atol=1e-3) < 1
    assert fidelity.budget_used(ref, ref + np.array([0, 1e-2, 0], np.float32), atol=1e-3) > 1
    # atol 0: exact match required where |ref| is 0.
    assert fidelity.budget_used(ref, ref, atol=0.0) == 0.0
    assert fidelity.budget_used(ref, ref + np.array([1e-9, 0, 0], np.float32), atol=0.0) == float("inf")


def test_letterbox_pads_with_black_and_keeps_aspect():
    img = np.full((50, 100, 3), 255, np.uint8)
    out = fidelity.letterbox(img, 128)
    assert out.shape == (128, 128, 3)
    assert out[0, 64].sum() == 0 and out[64, 64].min() == 255


@pytest.mark.skipif(not MANIFEST_PATH.exists(), reason="models/manifest.json not built")
def test_manifest_validates_and_matches_files():
    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    schema = json.loads(MANIFEST_SCHEMA_PATH.read_text(encoding="utf-8"))
    jsonschema.Draft202012Validator(schema, format_checker=jsonschema.FormatChecker()).validate(manifest)
    assert {m["id"] for m in manifest["models"]} == REQUIRED_IDS
    for m in manifest["models"]:
        assert m["review"]["status"] == "pending"
        assert "NOT RUN" not in m["conversion"].get("fidelity", ""), m["id"]
        path = MODELS_DIR / m["file"]
        if path.exists():
            assert upstream.sha256_file(path) == m["sha256"], m["id"]
    aura = next(m for m in manifest["models"] if m["id"] == "auraface-v1")
    assert aura["licence"]["trainingData"]["commercialUse"] == "unclear"


def test_web_sdk_pins_match_lab_upstream():
    """sdk-web ships two of the lab's upstream files to browsers; their URL and hash must agree with ours."""
    script = MODELS_DIR.parent / "sdk-web" / "scripts" / "fetch-models.mjs"
    if not script.exists():
        pytest.skip("sdk-web not present")
    text = script.read_text(encoding="utf-8")
    for key in upstream.WEB_SDK_ASSETS:
        src = upstream.SOURCES[key]
        assert src.url in text, key
        assert src.sha256 in text, key
