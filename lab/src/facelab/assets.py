"""Non-model assets: the canonical face used by the pose fit."""

from __future__ import annotations

import json
import re
from pathlib import Path

from facelab.upstream import SOURCES

CANONICAL_ID = "mp-canonical-face"
CANONICAL_FILE = "mp-canonical-face.json"


def build_canonical_face(dest: Path) -> dict:
    obj = SOURCES["canonical_face_obj"].path.read_text()
    vertices = [[float(v) for v in ln.split()[1:4]] for ln in obj.splitlines() if ln.startswith("v ")]
    if len(vertices) != 468:
        raise RuntimeError(f"canonical face: expected 468 vertices, got {len(vertices)}")
    pbtxt = SOURCES["geometry_pipeline_metadata"].path.read_text()
    basis = [
        {"landmark": int(m.group(1)), "weight": float(m.group(2))}
        for m in re.finditer(r"landmark_id:\s*(\d+)\s+weight:\s*([0-9.eE+-]+)", pbtxt)
    ]
    if not basis:
        raise RuntimeError("canonical face: no procrustes_landmark_basis entries")
    doc = {
        "id": CANONICAL_ID,
        "units": "cm",
        "frame": "MediaPipe face-geometry metric space, as in the upstream OBJ (vertex i = landmark i).",
        "vertices": vertices,
        "procrustesLandmarkBasis": basis,
    }
    dest.write_text(json.dumps(doc, separators=(",", ":")) + "\n", encoding="utf-8", newline="\n")
    return doc
