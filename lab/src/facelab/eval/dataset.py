"""Dataset manifest: what to evaluate, with labels, but never the data itself.

Format (YAML or JSON), formatVersion 1:

    formatVersion: 1
    name: example-pilot-2026q4           # free text
    illustrative: false               # true only for the committed public-domain fixtures
    consent: "DPA ref ..."            # REQUIRED unless illustrative: where the consent record lives
    root: D:/secure/face-eval/pilot   # images are resolved relative to this; must be OUTSIDE the repository
    subjects:
      - id: s0001
        attributes: {sex: F, ageBand: 30-39, skinTone: monk-6}   # any keys; used for groups and matching
    samples:
      - id: s0001-selfie-1
        subject: s0001
        path: s0001/selfie-1.jpg
        role: probe                   # probe (live capture) | reference (document / enrolled portrait)
        kind: selfie                  # selfie | document | portrait
        frontal: true                 # optional: head level, fixation mark at EYE height (pitch-offset study)
        cameraElevationDeg: 0.0       # optional: camera above (+) / below (-) eye level, in degrees
        labels: {EYES_OPEN: pass}     # optional ground truth per check id: pass | fail (annotator 1)
        labelsAnnotator2: {EYES_OPEN: fail}  # optional second annotator; disagreements become "ambiguous"

Genuine and impostor comparisons are derived from subject identity (protocol.py).
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path

import yaml

from facelab.paths import REPO_ROOT, TESTDATA_DIR, WORK_DIR


class DatasetError(ValueError):
    pass


@dataclass
class Sample:
    id: str
    subject: str
    path: Path
    role: str
    kind: str
    frontal: bool | None = None
    camera_elevation_deg: float | None = None
    labels: dict[str, str] = field(default_factory=dict)
    labels2: dict[str, str] = field(default_factory=dict)


@dataclass
class Dataset:
    name: str
    illustrative: bool
    root: Path
    subjects: dict[str, dict[str, str]]
    samples: list[Sample]


def _inside(path: Path, parent: Path) -> bool:
    try:
        path.resolve().relative_to(parent.resolve())
        return True
    except ValueError:
        return False


def check_output_dir(out: Path) -> None:
    """Results (embeddings caches, per-comparison scores) are derived biometric data: never inside the repo,
    except lab/work/, which is git-ignored."""
    if _inside(out, REPO_ROOT) and not _inside(out, WORK_DIR):
        raise DatasetError(f"output directory {out} is inside the repository; use lab/work/ or a path outside it")


def load(path: Path) -> Dataset:
    text = Path(path).read_text(encoding="utf-8")
    raw = json.loads(text) if str(path).endswith(".json") else yaml.safe_load(text)
    if raw.get("formatVersion") != 1:
        raise DatasetError("formatVersion must be 1")
    illustrative = bool(raw.get("illustrative", False))
    root = Path(raw["root"])
    if not root.is_absolute():
        root = (Path(path).parent / root).resolve()
    if _inside(root, REPO_ROOT) and not (illustrative and _inside(root, TESTDATA_DIR)):
        raise DatasetError(f"dataset root {root} is inside the repository; real data must live outside it")
    if not illustrative and not raw.get("consent"):
        raise DatasetError("'consent' is required for non-illustrative datasets")
    subjects = {s["id"]: {k: str(v) for k, v in (s.get("attributes") or {}).items()} for s in raw["subjects"]}
    samples = []
    seen = set()
    for s in raw["samples"]:
        if s["id"] in seen:
            raise DatasetError(f"duplicate sample id {s['id']}")
        seen.add(s["id"])
        if s["subject"] not in subjects:
            raise DatasetError(f"sample {s['id']}: unknown subject {s['subject']}")
        if s["role"] not in ("probe", "reference"):
            raise DatasetError(f"sample {s['id']}: role must be probe or reference")
        for key in ("labels", "labelsAnnotator2"):
            for check, lab in (s.get(key) or {}).items():
                if lab not in ("pass", "fail"):
                    raise DatasetError(f"sample {s['id']}: {key} {check}={lab} must be pass or fail")
        samples.append(
            Sample(
                s["id"],
                s["subject"],
                root / s["path"],
                s["role"],
                s.get("kind", "selfie" if s["role"] == "probe" else "document"),
                s.get("frontal"),
                s.get("cameraElevationDeg"),
                dict(s.get("labels") or {}),
                dict(s.get("labelsAnnotator2") or {}),
            )
        )
    return Dataset(raw.get("name", Path(path).stem), illustrative, root, subjects, samples)
