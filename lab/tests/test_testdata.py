"""Test-data invariants. Offline; checks the committed files against provenance.json."""

from __future__ import annotations

import hashlib
import json

import numpy as np
import pytest

from facelab.paths import TESTDATA_DIR
from facelab.testdata import documents, portraits

PROV = TESTDATA_DIR / "provenance.json"


def test_rotate_points_matches_numpy_rot90():
    img = np.zeros((3, 5), np.uint8)  # h=3, w=5
    img[1, 4] = 1  # (x=4, y=1)
    for rot in (90, 180, 270):
        r = np.rot90(img, k=-rot // 90)
        y, x = np.argwhere(r == 1)[0]
        got = documents._rotate_points(np.float32([[4, 1]]), rot, 5, 3)[0]
        assert (got[0], got[1]) == (x, y), rot


def test_portrait_ids_are_unique_and_pinned():
    ids = [p.id for p in portraits.PORTRAITS]
    assert len(ids) == len(set(ids))
    assert all(len(p.sha1) == 40 for p in portraits.PORTRAITS)


def test_pd_template_regex():
    assert portraits.PD_TEMPLATE.search("== License ==\n{{PD-USGov-NASA}}\n")
    assert not portraits.PD_TEMPLATE.search("{{Cc-by-sa-4.0}}")


@pytest.mark.skipif(not PROV.exists(), reason="test data not built")
def test_committed_files_match_provenance():
    data = json.loads(PROV.read_text(encoding="utf-8"))
    rows = data["portraits"] + data.get("derived", []) + data["documents"]
    assert len(data["portraits"]) == len(portraits.PORTRAITS)
    assert len(data["documents"]) == len(documents.DOCS)
    for r in rows:
        path = TESTDATA_DIR / r["file"]
        assert hashlib.sha256(path.read_bytes()).hexdigest() == r["sha256"], r["id"]
    licences = (TESTDATA_DIR / "LICENSES.md").read_text(encoding="utf-8")
    for r in rows:
        assert r["sha256"] in licences, r["id"]
    # Only listed files may exist under images/ and documents/.
    listed = {r["file"] for r in rows}
    on_disk = {f"{d}/{p.name}" for d in ("images", "documents") for p in (TESTDATA_DIR / d).iterdir()}
    assert on_disk == listed
