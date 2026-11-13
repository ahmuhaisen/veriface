"""Model pack: reproducible, self-verifying, manifest byte-identical. Uses a tiny fake model set."""

from __future__ import annotations

import hashlib
import json
import zipfile

import pytest

from facelab import pack, upstream


@pytest.fixture
def fake_models(tmp_path, monkeypatch):
    models = tmp_path / "models"
    models.mkdir()
    blobs = {"a.onnx": b"model-a", "b.json": b'{"x": 1}'}
    for name, data in blobs.items():
        (models / name).write_bytes(data)

    def entry(mid, name):
        return {
            "id": mid,
            "file": name,
            "sha256": hashlib.sha256(blobs[name]).hexdigest(),
            "upstream": {"name": f"up {mid}", "version": "1", "url": "https://example.invalid/x"},
            "licence": {"spdx": "Apache-2.0", "attribution": f"{mid}: attribution"},
        }

    manifest = {"models": [entry("auraface-v1", "a.onnx")], "assets": [entry("mp-canonical-face", "b.json")]}
    (models / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
    monkeypatch.setattr(pack, "MODELS_DIR", models)
    monkeypatch.setattr(pack, "MANIFEST_PATH", models / "manifest.json")
    for key in ("mediapipe_license", "auraface_license", *upstream.WEB_SDK_ASSETS):
        if key in ("mediapipe_license", "auraface_license") and not upstream.SOURCES[key].path.exists():
            pytest.skip("licence files not fetched (python -m facelab models fetch)")
    return models


def test_pack_is_reproducible_and_self_verifying(fake_models, tmp_path):
    z1 = pack.write_pack("9.9.9", tmp_path / "o1")
    z2 = pack.write_pack("9.9.9", tmp_path / "o2")
    assert z1.read_bytes() == z2.read_bytes()
    root = "veriface-models-9.9.9/"
    with zipfile.ZipFile(z1) as z:
        assert z.read(root + "models/manifest.json") == (fake_models / "manifest.json").read_bytes()
        for line in z.read(root + "SHA256SUMS").decode().splitlines():
            digest, path = line.split("  ", 1)
            assert hashlib.sha256(z.read(root + path)).hexdigest() == digest
        p = json.loads(z.read(root + "PACK.json"))
        assert (
            p["version"] == "9.9.9"
            and p["manifestSha256"] == hashlib.sha256((fake_models / "manifest.json").read_bytes()).hexdigest()
        )
        assert "licenses/models/auraface-v1/LICENSE.md" in {n.removeprefix(root) for n in z.namelist()}
        assert "auraface-v1: attribution" in z.read(root + "NOTICE").decode()
        tpl = z.read(root + "THIRD-PARTY-LICENSES.txt").decode()
        assert tpl.startswith("=" * 80 + "\n") and "\nLicense: Apache-2.0\n" in tpl
    sidecar = (tmp_path / "o1" / "veriface-models-9.9.9.zip.sha256").read_text()
    assert sidecar.split()[0] == hashlib.sha256(z1.read_bytes()).hexdigest()


def test_pack_refuses_a_tampered_model(fake_models, tmp_path):
    (fake_models / "a.onnx").write_bytes(b"tampered")
    with pytest.raises(pack.PackError, match="SHA-256"):
        pack.write_pack("9.9.9", tmp_path / "o")
