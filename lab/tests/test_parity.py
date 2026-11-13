"""L8 parity comparisons on synthetic API responses built from the goldens (no service needed)."""

from __future__ import annotations

import copy
import json

import pytest

from facelab import parity
from facelab.paths import GOLDEN_DIR

PIPE = GOLDEN_DIR / "pipeline"
pytestmark = pytest.mark.skipif(not PIPE.exists(), reason="goldens not generated")


def _golden(gid: str) -> dict:
    return json.loads((PIPE / f"{gid}.json").read_text(encoding="utf-8"))


def _check(cid: str, value, status: str = "pass") -> dict:
    return {"id": cid, "group": "x", "status": status, "severity": "error", "value": value, "unit": "x"}


def _perfect_selfie(g: dict) -> dict:
    f = g["faces"][0]
    w = g["image"]["width"]
    return {
        "checks": [
            _check("FACE_PRESENT", len(g["detections"]["full"])),
            _check("SINGLE_FACE", sum(1 for x in g["faces"] if x["iod"] >= 0.02 * w)),
            _check("INTER_EYE_DISTANCE", f["iod"]),
        ],
        "face": {"box": {"x": 0, "y": 0, "width": 1, "height": 1}, "landmarks5": f["landmarks5"],
                 "interEyeDistancePx": f["iod"], "pose": dict(f["pose"])},
    }  # fmt: skip


def test_perfect_selfie_passes_and_errors_fail():
    g = _golden("nasa-glover")
    r = parity.compare_selfie(_perfect_selfie(g), g)
    assert {k: v["status"] for k, v in r.items()} == dict.fromkeys(
        ["FACE_PRESENT", "SINGLE_FACE", "INTER_EYE_DISTANCE", "faceIOD", "landmarks5", "pose"], "pass"
    )
    bad = copy.deepcopy(_perfect_selfie(g))
    bad["face"]["pose"]["pitch"] += 5.0  # beyond the 2 degree pitch tolerance
    bad["face"]["landmarks5"][0][0] += 0.1 * g["faces"][0]["iod"]
    r = parity.compare_selfie(bad, g)
    assert r["pose"]["status"] == "fail" and r["landmarks5"]["status"] == "fail"


def test_stub_response_is_undetermined_not_fail():
    g = _golden("nasa-glover")
    stub = {"checks": [_check(c, None, "undetermined") for c in ("FACE_PRESENT", "SINGLE_FACE", "INTER_EYE_DISTANCE")],
            "face": None}  # fmt: skip
    statuses = {v["status"] for v in parity.compare_selfie(stub, g).values()}
    assert statuses == {"undetermined"}


def test_document_orientation_corners_and_portrait_box():
    g = _golden("doc-watkins-rot90")
    gt = g["documents"]
    quad = gt["mainPortraitQuads"][0]
    xs, ys = [p[0] for p in quad], [p[1] for p in quad]
    resp = _perfect_selfie(g)
    # Document check IOD is on the rectified portrait; FaceInfo IOD and landmarks are in input coordinates.
    resp["checks"] = [_check("PORTRAIT_FOUND", 1), _check("INTER_EYE_DISTANCE", g["faces"][0]["iodDocumentFrame"])]
    resp["document"] = {"detected": True, "confidence": 0.9, "corners": gt["cardCorners"], "orientationDegrees": 270}
    resp["portraitSourceBox"] = {"x": min(xs), "y": min(ys), "width": max(xs) - min(xs), "height": max(ys) - min(ys)}
    r = parity.compare_document(resp, g)
    assert {k: v["status"] for k, v in r.items()} == dict.fromkeys(
        [
            "PORTRAIT_FOUND",
            "orientation",
            "corners",
            "portraitSourceBox",
            "INTER_EYE_DISTANCE",
            "faceIOD",
            "landmarks5",
            "pose",
        ],
        "pass",
    )
    resp["document"]["orientationDegrees"] = 90  # the image was rotated 90 cw, so 270 cw makes it upright
    assert parity.compare_document(resp, g)["orientation"]["status"] == "fail"


def test_document_landmarks_use_document_tolerance():
    g = _golden("doc-watkins")
    resp = copy.deepcopy(_perfect_selfie(g))  # deep copy: the helper shares the golden's lists
    resp["checks"] = [_check("PORTRAIT_FOUND", 1), _check("INTER_EYE_DISTANCE", g["faces"][0]["iodDocumentFrame"])]
    resp["document"] = {
        "detected": True,
        "confidence": 1.0,
        "corners": g["documents"]["cardCorners"],
        "orientationDegrees": 0,
    }
    resp["face"]["landmarks5"][0][0] += 0.025 * g["faces"][0]["iod"]  # > selfie 0.02, < document 0.03
    assert parity.compare_document(resp, g)["landmarks5"]["status"] == "pass"
    assert parity.compare_selfie(resp, g)["landmarks5"]["status"] == "fail"


def test_flat_scan_detection_is_not_expected():
    g = _golden("doc-ochoa-scan")
    resp = _perfect_selfie(g)
    resp["checks"] = [_check("PORTRAIT_FOUND", 1)]
    resp["document"] = {"detected": False, "confidence": 0.0, "corners": None, "orientationDegrees": 0}
    r = parity.compare_document(resp, g)
    assert r["orientation"]["status"] == "n/a" and r["corners"]["status"] == "n/a"


def _verify_resp(score, outcome="verified", probe="accepted", ref="accepted", jpeg_fail=False):
    checks = [{"id": "COMPRESSION_QUALITY", "status": "fail" if jpeg_fail else "pass"}]
    return {
        "score": score,
        "outcome": outcome,
        "probe": {"verdict": probe},
        "reference": {"verdict": ref, "checks": checks},
    }


def test_verification_score_rules():
    assert parity.verification_delta("p", "r", _verify_resp(0.50), 0.515)["status"] == "pass"
    assert parity.verification_delta("p", "r", _verify_resp(0.50), 0.53)["status"] == "fail"
    heavy = parity.verification_delta("p", "r", _verify_resp(0.50, jpeg_fail=True), 0.53)
    assert heavy["status"] == "pass" and heavy["tolerance"] == parity.SCORE_TOL_HEAVY_JPEG
    gated = parity.verification_delta("p", "r", _verify_resp(None, "retake", ref="rejected"), 0.5)
    assert gated["status"] == "retake"
    # A retake without any quality rejection is unexplained: it must stay undetermined (blocks --require-complete).
    assert parity.verification_delta("p", "r", _verify_resp(None, "retake"), 0.5)["status"] == "undetermined"
    assert parity.verification_delta("p", "r", _verify_resp(0.5), None)["status"] == "n/a"
