"""Compare two golden sets field by field (e.g. before/after a runtime pin change).

    python -m facelab.experiments.golden_drift <old golden dir> [<new golden dir>]   (default new: testdata/golden)

Reports, over fixtures present in both: detection counts and box differences, face counts, landmark / landmarks5
error in IOD, blendshape and pose differences, embedding cosine, segmentation mask agreement, and the max
difference of every model-level .npy output. Prints the worst case per field against the golden tolerances.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import cv2
import numpy as np

from facelab.paths import GOLDEN_DIR


def _iou(a, b) -> float:
    ax, ay, aw, ah = a
    bx, by, bw, bh = b
    ix = max(0.0, min(ax + aw, bx + bw) - max(ax, bx))
    iy = max(0.0, min(ay + ah, by + bh) - max(ay, by))
    u = aw * ah + bw * bh - ix * iy
    return ix * iy / u if u else 0.0


def compare(old: Path, new: Path) -> dict:
    worst: dict[str, tuple[float, str]] = {}

    def note(key: str, value: float, where: str, larger_is_worse: bool = True) -> None:
        cur = worst.get(key)
        if cur is None or (value > cur[0] if larger_is_worse else value < cur[0]):
            worst[key] = (value, where)

    mismatches = []
    for p in sorted((new / "pipeline").glob("*.json")):
        q = old / "pipeline" / p.name
        if not q.exists():
            continue
        a, b = json.loads(q.read_text(encoding="utf-8")), json.loads(p.read_text(encoding="utf-8"))
        fid = p.stem
        for v in ("short", "full"):
            da, db = a["detections"][v], b["detections"][v]
            if len(da) != len(db):
                mismatches.append(f"{fid} detections.{v}: {len(da)} -> {len(db)}")
                continue
            for x, y in zip(da, db, strict=True):
                note(f"detections.{v}.boxIoU(min)", _iou(x["box"], y["box"]), fid, larger_is_worse=False)
                note(f"detections.{v}.boxEdgePx", float(np.abs(np.array(x["box"]) - np.array(y["box"])).max()), fid)
                note(f"detections.{v}.score", abs(x["score"] - y["score"]), fid)
                kd = np.linalg.norm(np.array(x["keypoints"]) - np.array(y["keypoints"]), axis=1).max()
                note(f"detections.{v}.keypointsPx", float(kd), fid)
        if len(a["faces"]) != len(b["faces"]):
            mismatches.append(f"{fid} faces: {len(a['faces'])} -> {len(b['faces'])}")
            continue
        for k, (fa, fb) in enumerate(zip(a["faces"], b["faces"], strict=True)):
            w = f"{fid}#{k}"
            iod = fa["iod"]
            la, lb = np.array(fa["landmarks"])[:, :2], np.array(fb["landmarks"])[:, :2]
            e = np.linalg.norm(la - lb, axis=1) / iod
            note("landmarks.meanIodFrac", float(e.mean()), w)
            note("landmarks.maxIodFrac", float(e.max()), w)
            e5 = np.linalg.norm(np.array(fa["landmarks5"]) - np.array(fb["landmarks5"]), axis=1) / iod
            note("landmarks5.iodFrac", float(e5.max()), w)
            bs = max(abs(fa["blendshapes"][n] - fb["blendshapes"][n]) for n in fa["blendshapes"])
            note(f"blendshapes.abs[{fb['source']}]", bs, w)
            for ang in ("yaw", "pitch", "roll"):
                note(f"pose.{ang}Deg", abs(fa["pose"][ang] - fb["pose"][ang]), w)
            note("embedding.cosine(min)", float(np.dot(fa["embedding"], fb["embedding"])), w, larger_is_worse=False)
            sa, sb = fa.get("segmentation"), fb.get("segmentation")
            if sa and sb and sa["roi"] == sb["roi"]:
                ma = cv2.imread(str(old / sa["maskFile"]), cv2.IMREAD_UNCHANGED)
                mb = cv2.imread(str(new / sb["maskFile"]), cv2.IMREAD_UNCHANGED)
                if ma.ndim == 3:
                    ma = ma[..., 0]
                if mb.ndim == 3:
                    mb = mb[..., 0]
                note("segmentation.maskAgreement(min)", float((ma == mb).mean()), w, larger_is_worse=False)
            elif sa and sb:
                note("segmentation.roiChangedPx", float(np.abs(np.array(sa["roi"]) - np.array(sb["roi"])).max()), w)
    model = {}
    for f in sorted((new / "model").rglob("*.npy")):
        g = old / f.relative_to(new)
        if g.exists():
            x, y = np.load(g).astype(np.float64), np.load(f).astype(np.float64)
            if x.shape == y.shape:
                key = f.name.split(".", 1)[1].removesuffix(".npy")  # model-id-less output name
                d = float(np.abs(x - y).max())
                model[f"{f.parent.name}/{f.name}"] = d
                note(f"model.{f.name.split('.')[0]}.{key}", d, f.parent.name)
    return {"mismatches": mismatches, "worst": worst, "modelLevel": model}


def main() -> int:
    old = Path(sys.argv[1])
    new = Path(sys.argv[2]) if len(sys.argv) > 2 else GOLDEN_DIR
    r = compare(old, new)
    print("count mismatches:", r["mismatches"] or "none")
    for k, (v, w) in sorted(r["worst"].items()):
        print(f"  {k:48s} {v:.6g}   ({w})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
