"""Golden-generator tests. Need the telemetry-free MediaPipe venv (lab/.venv-mp); skipped elsewhere."""

from __future__ import annotations

import json

import numpy as np
import pytest

golden = pytest.importorskip("facelab.golden", reason="needs lab/.venv-mp (mediapipe 0.10.21)")

from facelab.paths import GOLDEN_DIR  # noqa: E402
from facelab.testdata import provenance  # noqa: E402


def test_seg_roi_rule_clamps_without_padding():
    pts = np.zeros((478, 3))
    pts[468, :2], pts[473, :2] = (10, 50), (30, 50)  # IOD 20, near the left edge
    x, y, w, h = golden.seg_roi(pts, 200, 300)
    assert (x, y) == (0, 0)  # centre (20, 60), side 120 -> clamped at 0
    assert (w, h) == (80, 120)


def test_generator_refuses_unlisted_images():
    with pytest.raises(golden.GoldenError, match="provenance"):
        golden.build(["some-real-selfie"])


@pytest.mark.skipif(not (GOLDEN_DIR / "pipeline").exists(), reason="goldens not generated")
def test_goldens_cover_exactly_the_provenance_images():
    files = {p.stem for p in (GOLDEN_DIR / "pipeline").glob("*.json")}
    assert files == set(provenance.all_images())
    for stem in files:
        g = json.loads((GOLDEN_DIR / "pipeline" / f"{stem}.json").read_text(encoding="utf-8"))
        assert g["formatVersion"] == golden.FORMAT_VERSION
        for f in g["faces"]:
            assert len(f["landmarks"]) == 478 and len(f["embedding"]) == 512
            assert np.linalg.norm(f["embedding"]) == pytest.approx(1.0, abs=1e-4)
            assert f["pose"]["yaw"] == pytest.approx(f["eulerRaw"]["b"], abs=1e-3)


def test_document_frame_removes_card_tilt():
    import math

    from facelab import pose
    from facelab.testdata.documents import CARD_H, CARD_W

    verts, _ = pose.canonical()
    rot = pose.matrix_from_pose(yaw=6.0, pitch=-4.0, roll=2.0)
    obs = 3.0 * verts @ rot.T + np.array([300.0, -250.0, 0.0])  # face on the upright card, card px
    face = np.zeros((478, 3))
    face[:468] = np.column_stack([obs[:, 0], -obs[:, 1], -obs[:, 2]])
    upright = [[0, 0], [CARD_W, 0], [CARD_W, CARD_H], [0, CARD_H]]
    assert golden.to_document_frame(face, upright) == pytest.approx(face, abs=1e-6)
    # Tilt card + face together by 7 degrees clockwise in the image, scale 1.3, then shift.
    t, s = math.radians(7.0), 1.3
    rmat = np.array([[math.cos(t), -math.sin(t)], [math.sin(t), math.cos(t)]]) * s

    def photo(xy):
        return np.asarray(xy) @ rmat.T + [200.0, 150.0]

    tilted = face.copy()
    tilted[:, :2] = photo(face[:, :2])
    tilted[:, 2] = face[:, 2] * s
    corners = photo(np.array(upright, float)).tolist()
    in_photo = pose.fit_weak_perspective(tilted)
    in_card = pose.fit_weak_perspective(golden.to_document_frame(tilted, corners))
    assert in_photo["roll"] == pytest.approx(2.0 + 7.0, abs=1e-6)  # the tilt shows up as roll in the photo
    assert in_card["roll"] == pytest.approx(2.0, abs=1e-6)  # and is removed in the document frame
    assert in_card["yaw"] == pytest.approx(6.0, abs=1e-6) and in_card["pitch"] == pytest.approx(-4.0, abs=1e-6)
