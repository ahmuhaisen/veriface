"""L8 ``facelab parity``: run the fixtures through the service API and compare with the lab's expectations.

Writes (default lab/work/parity/, git-ignored):
  scores.csv          probe,reference,score for every pair in the fixture protocol (via /v1/verifications)
  predictions.jsonl   {"sample", "checks": {CHECK_ID: status}} per sample (via /v1/quality, /v1/document-portraits)
  responses/          raw API responses, one JSON per call
  parity.json/.md     per-field comparison against testdata/golden and the lab reference scorer
  eval/               the L7 evaluation report computed on the SERVICE's scores and predictions

Every comparison ends as pass, fail, undetermined (the service gave no value: e.g. the stub engine), or
n/a (nothing to compare against). The release gate is: no fail, and with --require-complete no undetermined.
"""

from __future__ import annotations

import csv
import json
import os
import uuid
from pathlib import Path

import numpy as np
import requests

from facelab.eval import dataset as dsm
from facelab.eval import protocol
from facelab.paths import GOLDEN_DIR, TESTDATA_DIR, WORK_DIR

SCORE_TOL = 0.02  # |service score - lab reference score|
# Heavy JPEG: decoders differ on strong blocking (service pure-Rust vs libjpeg-turbo), so a reference whose
# COMPRESSION_QUALITY check fails gets this tolerance (lead decision 2026-10-01; observed max 0.032 at quality 8).
SCORE_TOL_HEAVY_JPEG = 0.04
IOD_REL_TOL = 0.03
CORNER_TOL_DIAG = 0.02  # max corner error as a fraction of the card diagonal
# "retake": no score because the service asked for a retake AND its own response shows the probe or the
# reference was rejected by quality checks; expected behaviour, so it does not block the release gate.
STATUSES = ("pass", "fail", "undetermined", "retake", "n/a")


class Client:
    def __init__(self, base: str, key: str, profile: str | None, timeout: float = 120.0):
        self.base = base.rstrip("/")
        self.s = requests.Session()
        self.s.headers["X-API-Key"] = key
        self.profile = profile
        self.timeout = timeout

    def _post(self, path: str, files: dict[str, Path], params: dict | None = None) -> dict:
        q = dict(params or {})
        if self.profile:
            q["profile"] = self.profile
        handles = {k: (p.name, p.open("rb"), "image/jpeg") for k, p in files.items()}
        try:
            r = self.s.post(
                f"{self.base}{path}",
                params=q,
                files=handles,
                headers={"X-Request-Id": f"parity-{uuid.uuid4()}"},
                timeout=self.timeout,
            )
        finally:
            for _, fh, _ in handles.values():
                fh.close()
        if r.status_code != 200:
            raise RuntimeError(f"{path} -> {r.status_code}: {r.text[:300]}")
        return r.json()

    def info(self) -> dict:
        r = self.s.get(f"{self.base}/v1/info", timeout=self.timeout)
        r.raise_for_status()
        return r.json()

    def quality(self, p: Path) -> dict:
        return self._post("/v1/quality", {"image": p})

    def document(self, p: Path) -> dict:
        return self._post("/v1/document-portraits", {"document": p})

    def verify(self, probe: Path, reference: Path, kind: str) -> dict:
        return self._post("/v1/verifications", {"probe": probe, "reference": reference}, {"referenceKind": kind})


def _check(resp: dict, check_id: str) -> dict | None:
    return next((c for c in resp.get("checks", []) if c["id"] == check_id), None)


def _value(resp: dict, check_id: str):
    c = _check(resp, check_id)
    return None if c is None or c["status"] in ("undetermined", "skipped") else c["value"]


def _cmp(ok: bool | None) -> str:
    return "undetermined" if ok is None else ("pass" if ok else "fail")


def compare_selfie(resp: dict, golden: dict) -> dict[str, dict]:
    out: dict[str, dict] = {}
    w = golden["image"]["width"]
    gfaces = golden["faces"]
    n_full = len(golden["detections"]["full"])
    v = _value(resp, "FACE_PRESENT")
    out["FACE_PRESENT"] = {"status": _cmp(None if v is None else v == n_full), "service": v, "expected": n_full}
    n_single = sum(1 for f in gfaces if f["iod"] >= 0.02 * w)
    v = _value(resp, "SINGLE_FACE")
    out["SINGLE_FACE"] = {"status": _cmp(None if v is None else v == n_single), "service": v, "expected": n_single}
    if not gfaces:
        out["face"] = {"status": "n/a" if resp.get("face") is None else "fail", "note": "golden has no face"}
        return out
    out.update(compare_geometry(resp, golden))
    return out


def compare_geometry(resp: dict, golden: dict, document: bool = False) -> dict[str, dict]:
    """Primary-face geometry vs the golden's faces[0].

    Landmarks, box and FaceInfo.interEyeDistancePx are in input-image coordinates. For document portraits the
    INTER_EYE_DISTANCE *check* is measured on the rectified, upright portrait (checks.yaml), so it is compared with
    the golden's ``iodDocumentFrame``, and ``pose`` with the golden's document-frame pose.
    """
    out: dict[str, dict] = {}
    g = golden["faces"][0]
    iod = g["iod"]
    check_iod = g["iodDocumentFrame"] if document else iod
    v = _value(resp, "INTER_EYE_DISTANCE")
    out["INTER_EYE_DISTANCE"] = {
        "status": _cmp(None if v is None else abs(v - check_iod) / check_iod <= IOD_REL_TOL),
        "service": v,
        "expected": check_iod,
        "frame": "document" if document else "input",
        "tolerance": f"rel {IOD_REL_TOL}",
    }
    face = resp.get("face")
    tol = golden["tolerances"]
    if face is None:
        out["faceIOD"] = {"status": "undetermined"}
        out["landmarks5"] = {"status": "undetermined"}
        out["pose"] = {"status": "undetermined"}
        return out
    fv = face.get("interEyeDistancePx")
    out["faceIOD"] = {
        "status": _cmp(None if fv is None else abs(fv - iod) / iod <= IOD_REL_TOL),
        "service": fv,
        "expected": iod,
        "frame": "input",
        "tolerance": f"rel {IOD_REL_TOL}",
    }
    l5_tol = tol["landmarks5"]["iodFracDocument"] if document else tol["landmarks5"]["iodFrac"]
    d = np.linalg.norm(np.array(face["landmarks5"]) - np.array(g["landmarks5"]), axis=1) / iod
    out["landmarks5"] = {
        "status": _cmp(bool(d.max() <= l5_tol)),
        "maxIodFrac": round(float(d.max()), 4),
        "tolerance": l5_tol,
    }
    pose = face.get("pose")
    if pose is None:
        out["pose"] = {"status": "undetermined"}
    else:
        diffs = {k: round(abs(pose[k] - g["pose"][k]), 3) for k in ("yaw", "pitch", "roll")}
        lim = {"yaw": tol["pose"]["yawDeg"], "pitch": tol["pose"]["pitchDeg"], "roll": tol["pose"]["rollDeg"]}
        out["pose"] = {"status": _cmp(all(diffs[k] <= lim[k] for k in diffs)), "absDiffDeg": diffs, "tolerance": lim}
    return out


def _inside(pt, quad) -> bool:
    import cv2

    return cv2.pointPolygonTest(np.float32(quad), (float(pt[0]), float(pt[1])), False) >= 0


def _is_flat_scan(card_corners: list, image: dict, slack_px: float = 2.0) -> bool:
    """True when the card fills the whole image (a flat scan): there is no outline to detect."""
    c = np.asarray(card_corners, float)
    w, h = image["width"], image["height"]
    full = np.array([[0, 0], [w, 0], [w, h], [0, h]], float)
    return bool(np.abs(c - full).max() <= slack_px)


def compare_document(resp: dict, golden: dict) -> dict[str, dict]:
    gt = golden["documents"]
    out: dict[str, dict] = {}
    expected_found = gt["mainPortraits"] >= 1
    v = _value(resp, "PORTRAIT_FOUND")
    out["PORTRAIT_FOUND"] = {
        "status": _cmp(None if v is None else (v >= 1) == expected_found),
        "service": v,
        "expected": f">= 1: {expected_found} (main {gt['mainPortraits']}, ghost {gt['ghostPortraits']})",
    }
    if not expected_found:
        return out
    doc = resp.get("document") or {}
    if _is_flat_scan(gt["cardCorners"], golden["image"]):
        # The card fills the image: no outline to find. The contract treats this as a tight crop
        # (DOCUMENT_DETECTED low is a warning), so detection is not expected.
        note = "flat scan: no card outline in the image"
        out["orientation"] = {"status": "n/a", "note": note}
        out["corners"] = {"status": "n/a", "note": note}
    else:
        want_rot = (360 - gt["rotationCw"]) % 360
        got_rot = doc.get("orientationDegrees")
        # Only a detected document's orientation is meaningful (the stub reports 0 with detected=false).
        out["orientation"] = {
            "status": "undetermined" if not doc.get("detected") else _cmp(got_rot == want_rot),
            "service": got_rot,
            "expected": want_rot,
        }
        corners = doc.get("corners")
        if not corners:
            out["corners"] = {"status": "undetermined"}
        else:
            gtc = np.array(gt["cardCorners"])
            diag = float(np.linalg.norm(gtc[2] - gtc[0]))
            err = float(np.linalg.norm(np.array(corners) - gtc, axis=1).max()) / diag
            out["corners"] = {
                "status": _cmp(err <= CORNER_TOL_DIAG),
                "maxErrDiagFrac": round(err, 4),
                "tolerance": CORNER_TOL_DIAG,
            }
    box = resp.get("portraitSourceBox")
    if not box:
        out["portraitSourceBox"] = {"status": "undetermined"}
    else:
        c = (box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
        out["portraitSourceBox"] = {
            "status": _cmp(_inside(c, gt["mainPortraitQuads"][0])),
            "rule": "box centre inside a main-portrait quad",
        }
    if golden["faces"]:
        out.update(compare_geometry(resp, golden, document=True))
    return out


def verification_delta(probe: str, reference: str, resp: dict, lab_score: float | None) -> dict:
    """One pair: pass/fail on the score delta, or retake / undetermined / n/a when there is nothing to compare."""
    score = resp["score"]
    row: dict = {"probe": probe, "reference": reference, "service": score, "outcome": resp["outcome"]}
    if lab_score is None:
        return {**row, "status": "n/a", "lab": None, "note": "lab reference finds no face"}
    row["lab"] = round(lab_score, 4)
    if score is None:
        rejected = (
            resp.get("probe", {}).get("verdict") == "rejected" or resp.get("reference", {}).get("verdict") == "rejected"
        )
        if resp["outcome"] == "retake" and rejected:
            return {**row, "status": "retake", "note": "quality-gated retake (probe or reference rejected)"}
        return {**row, "status": "undetermined"}
    heavy_jpeg = any(
        c["id"] == "COMPRESSION_QUALITY" and c["status"] == "fail" for c in resp.get("reference", {}).get("checks", [])
    )
    tol = SCORE_TOL_HEAVY_JPEG if heavy_jpeg else SCORE_TOL
    d = abs(score - lab_score)
    return {**row, "status": _cmp(d <= tol), "absDelta": round(d, 5), "tolerance": tol}


def run(service: str, key: str, profile: str | None, out: Path, require_complete: bool = False) -> dict:
    from facelab.eval import run as evalrun  # needs lab/.venv-mp (MediaPipe scorer); comparisons below do not

    dsm.check_output_dir(out)
    (out / "responses").mkdir(parents=True, exist_ok=True)
    client = Client(service, key, profile)
    info = client.info()
    manifest = evalrun.fixtures_manifest(out / "manifest.yaml")
    ds = dsm.load(manifest)
    by_id = {s.id: s for s in ds.samples}

    # 1. Per-sample assessment.
    preds, fields = {}, {}
    for s in ds.samples:
        resp = client.document(s.path) if s.role == "reference" else client.quality(s.path)
        (out / "responses" / f"{s.id}.json").write_text(json.dumps(resp, indent=1), encoding="utf-8")
        preds[s.id] = {c["id"]: c["status"] for c in resp["checks"]}
        golden = json.loads((GOLDEN_DIR / "pipeline" / f"{s.id}.json").read_text(encoding="utf-8"))
        fields[s.id] = compare_document(resp, golden) if s.role == "reference" else compare_selfie(resp, golden)
    with (out / "predictions.jsonl").open("w", encoding="utf-8", newline="\n") as f:
        for sid, checks in preds.items():
            f.write(json.dumps({"sample": sid, "checks": checks}) + "\n")

    # 2. Every probe x reference pair via the verification endpoint; lab reference scores alongside.
    comps = protocol.build(ds, {s.id for s in ds.samples}, "all")
    lab = evalrun._score_all(ds, out)
    score_rows, deltas = [], []
    for p, r in zip(comps.probe, comps.reference, strict=True):
        resp = client.verify(by_id[p].path, by_id[r].path, "document")
        score = resp["score"]
        score_rows.append((p, r, "" if score is None else score, resp["outcome"], resp["decision"]))
        lp, lr = lab[p], lab[r]
        lab_score = float(lp.embedding @ lr.embedding) if lp.face_found and lr.face_found else None
        deltas.append(verification_delta(p, r, resp, lab_score))
    with (out / "scores.csv").open("w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["probe", "reference", "score", "outcome", "decision"])
        w.writerows(score_rows)

    # 3. Summary.
    summary: dict[str, dict[str, int]] = {}
    for sample_fields in fields.values():
        for name, res in sample_fields.items():
            summary.setdefault(name, dict.fromkeys(STATUSES, 0))[res["status"]] += 1
    summary["verificationScore"] = dict.fromkeys(STATUSES, 0)
    for d in deltas:
        summary["verificationScore"][d["status"]] += 1
    fails = sum(v["fail"] for v in summary.values())
    undetermined = sum(v["undetermined"] for v in summary.values())
    # Quality-gated retakes are counted as 'retake', not 'undetermined', so they never block the gate.
    gate = fails == 0 and (not require_complete or undetermined == 0)
    result = {
        "service": service,
        "engine": info,
        "profile": profile or "(key default)",
        "summary": summary,
        "gate": {"passed": gate, "fails": fails, "undetermined": undetermined, "requireComplete": require_complete},
        "samples": fields,
        "verification": deltas,
    }
    (out / "parity.json").write_text(json.dumps(result, indent=1) + "\n", encoding="utf-8")
    (out / "parity.md").write_text(render(result), encoding="utf-8")

    # 4. The L7 evaluation on the service's own scores and predictions (pairs without a score are dropped).
    evalrun.run(manifest, out / "eval", (), out / "scores.csv", out / "predictions.jsonl")
    return result


def render(r: dict) -> str:
    L = [f"# Parity report: {r['engine']['version']}\n"]
    if r["engine"]["version"].endswith("+stub"):
        L.append(
            "> The service runs the **stub engine**: every value is undetermined by design. This report proves "
            "the plumbing (API calls, files, comparisons), not the pipeline.\n"
        )
    g = r["gate"]
    L.append(
        f"Service {r['service']}, profile {r['profile']}. Gate: **{'PASSED' if g['passed'] else 'FAILED'}** "
        f"({g['fails']} fail, {g['undetermined']} undetermined, require complete: {g['requireComplete']}).\n"
    )
    L.append("| field | pass | fail | undetermined | retake | n/a |\n|---|---|---|---|---|---|")
    for name, c in r["summary"].items():
        L.append(f"| {name} | {c['pass']} | {c['fail']} | {c['undetermined']} | {c['retake']} | {c['n/a']} |")
    L.append("\n## Failures\n")
    any_fail = False
    for sid, fs in r["samples"].items():
        for name, res in fs.items():
            if res["status"] == "fail":
                any_fail = True
                L.append(f"- {sid} {name}: {json.dumps({k: v for k, v in res.items() if k != 'status'})}")
    for d in r["verification"]:
        if d["status"] == "fail":
            any_fail = True
            L.append(f"- {d['probe']} vs {d['reference']}: service {d['service']} vs lab {d['lab']}")
    if not any_fail:
        L.append("None.")
    L.append(
        "\n## Tolerances\n\n"
        "- Selfie: FACE_PRESENT equals the golden full-range detection count; SINGLE_FACE equals golden faces with "
        "IOD >= 0.02 x width; INTER_EYE_DISTANCE and FaceInfo.interEyeDistancePx (faceIOD) within 3% of the golden "
        "IOD; landmarks5 within 0.02 IOD and pose within the golden tolerances (testdata/golden/README.md).\n"
        "- Document (when the golden found the portrait): the INTER_EYE_DISTANCE check is measured on the rectified, "
        "upright portrait and compared with the golden's iodDocumentFrame; faceIOD and landmarks5 are input-image "
        "coordinates; landmarks5 within 0.03 IOD (two independent rectify/resample paths at ~50 px IOD); pose in the "
        "document frame. PORTRAIT_FOUND >= 1 iff the card has a main portrait; orientation = (360 - applied "
        f"rotation) mod 360; corners within {CORNER_TOL_DIAG} of the card diagonal (n/a for flat scans: no outline); "
        "portraitSourceBox centre inside the main portrait.\n"
        f"- Verification: |service score - lab reference score| <= {SCORE_TOL}, or <= {SCORE_TOL_HEAVY_JPEG} when "
        "the reference fails COMPRESSION_QUALITY (heavy JPEG: decoder differences). No score with outcome retake "
        "and a quality-rejected probe or reference = 'retake' (expected; does not block the gate).\n"
    )
    return "\n".join(L) + "\n"


def key_from_env(var: str) -> str:
    key = os.environ.get(var)
    if not key:
        # Convenience for local dev: the console's git-ignored .env.local.
        env_file = TESTDATA_DIR.parent / "tools" / "console" / ".env.local"
        if env_file.exists():
            for line in env_file.read_text(encoding="utf-8").splitlines():
                if line.startswith(f"{var}="):
                    key = line.split("=", 1)[1].strip()
    if not key:
        raise SystemExit(f"API key not found: set the {var} environment variable")
    return key


DEFAULT_OUT = WORK_DIR / "parity"
