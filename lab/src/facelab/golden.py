"""L5 ``facelab golden``: reference outputs for the Rust golden tests (format: testdata/golden/README.md).

Pipeline-level goldens come from the official MediaPipe Tasks runs (plus AuraFace via ONNX Runtime on the
shipped model). Model-level goldens are the raw tensors of the shipped ONNX models on fixed, saved inputs.

Only images listed in testdata/provenance.json (public-domain or synthetic) are accepted.
"""

from __future__ import annotations

import hashlib
import json
import math
from importlib.metadata import version

import cv2
import numpy as np

from facelab import __version__, align, measures, mptasks, pose
from facelab.fidelity import LANDMARK_SUBSET
from facelab.paths import GOLDEN_DIR, MANIFEST_PATH, TESTDATA_DIR
from facelab.ref import blazeface, pipeline
from facelab.ref.warp import RotatedRect, normalise, warp
from facelab.testdata import provenance

FORMAT_VERSION = 2  # v2 (2026-10-01): `pose` is the weak-perspective fit; MediaPipe matrix angles informational
MODEL_LEVEL_IDS = ["nasa-glover", "nasa-almansoori", "nasa-hoshide", "doc-watkins"]
SEG_CLASSES = ["background", "hair", "body-skin", "face-skin", "clothes", "others"]
NUM_FACES = 4

# Segmentation ROI rule (THE pipeline rule; README section "Segmentation ROI").
SEG_SIDE_IOD = 6.0
SEG_CENTRE_DOWN_IOD = 0.5
# Fallback landmarker crop when FaceLandmarker finds no face on the whole image.
CROP_SIDE_BOX = 2.2

TOLERANCES = {
    "detection": {"countExact": True, "boxIoU": 0.90, "boxEdgePx": 1.5, "score": 0.03, "keypointsIodFrac": 0.05},
    "landmarks": {"meanIodFrac": 0.015, "maxIodFrac": 0.05},
    # Document portraits pass through two independent rectify/resample paths (service rectifier vs ground-truth
    # warp) at ~50 px IOD: worst observed 0.0296 IOD (2026-10-01). Selfies stay at 0.02.
    "landmarks5": {"iodFrac": 0.02, "iodFracDocument": 0.03},
    # Crop-sourced faces (full-range fallback) see a re-cropped image: decoder differences of 3-5 grey levels and a
    # 1 px crop truncation move blendshapes up to ~0.06 while landmarks stay well within tolerance (vision agent).
    "blendshapes": {"abs": 0.05, "absCropSourced": 0.07},
    "illumination": {"LUMINANCE_MEAN": 0.01, "UNDER_EXPOSURE": 0.02, "OVER_EXPOSURE": 0.02},
    "pose": {"yawDeg": 2.0, "pitchDeg": 2.0, "rollDeg": 1.0},
    "embedding": {"cosineMin": 0.995},
    "segmentation": {"maskAgreementMin": 0.97, "classFractionAbs": 0.015},
}
MODEL_TOLERANCES = {"default": {"atol": 1e-4, "rtol": 1e-5}, "landmarks": {"atol": 1e-3, "rtol": 1e-5}}

# Sign checks, not precision checks: MediaPipe is not exactly mirror-equivariant (flip asymmetry up to 1.04
# degrees measured on these fixtures), so the tolerance equals the pose golden tolerance.
INVARIANTS = {
    "flip-horizontal": {"yaw": "negated", "roll": "negated", "pitch": "equal", "toleranceDeg": 2.0},
    "rotate-cw-15": {"yaw": "equal", "pitch": "equal", "roll": "plus 15", "toleranceDeg": 2.0},
    # Position invariance: the bottom placement must match the top placement of the same face.
    "canvas-bottom": {"yaw": "equal", "pitch": "equal", "roll": "equal", "toleranceDeg": 1.5},
}


class GoldenError(RuntimeError):
    pass


def _r(v, nd=3):
    return [_r(x, nd) for x in v] if isinstance(v, list | tuple | np.ndarray) else round(float(v), nd)


def _sha256(path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _load(row: dict) -> np.ndarray:
    path = TESTDATA_DIR / row["file"]
    if _sha256(path) != row["sha256"]:
        raise GoldenError(f"{row['id']}: file does not match provenance.json")
    return cv2.cvtColor(cv2.imread(str(path)), cv2.COLOR_BGR2RGB)


def _save_png(path, rgb: np.ndarray) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if rgb.ndim == 3 and rgb.shape[2] == 1:
        rgb = rgb[:, :, 0]  # single-channel (e.g. class-id masks): written as 8-bit greyscale, mode L
    data = rgb if rgb.ndim == 2 else cv2.cvtColor(rgb, cv2.COLOR_RGB2BGR)
    cv2.imwrite(str(path), data, [cv2.IMWRITE_PNG_COMPRESSION, 9])


# Where each detection set comes from. mediapipe 0.10.21 (the telemetry-free pin) cannot run the full-range model
# through Tasks FaceDetector (it builds short-range anchors only), so full-range uses the lab reference pipeline:
# the shipped ONNX + the decode in the manifest notes, which matched MediaPipe Tasks 1.0.1 full-range on every
# fixture (counts exact, keypoints <= 0.31 px, box IoU >= 0.92 limited by Tasks' integer boxes; 2026-10-01).
DETECTION_SOURCES = {
    "short": "mediapipe-tasks FaceDetector (integer boxes)",
    "full": "lab reference: mp-face-detector-full ONNX + manifest-notes decode (float boxes)",
}


def _detections(rgb: np.ndarray, variant: str) -> list[dict]:
    h, w = rgb.shape[:2]
    out = []
    if variant == "short":
        for d in mptasks.detect_faces(rgb, "short"):
            b = d.bounding_box
            out.append(
                {
                    "box": [b.origin_x, b.origin_y, b.width, b.height],
                    "score": round(d.categories[0].score, 6),
                    "keypoints": [[round(k.x * w, 3), round(k.y * h, 3)] for k in d.keypoints],
                }
            )
    else:
        out = [d.to_json() for d in pipeline.detect(rgb, "full")[0]]
    return sorted(out, key=lambda d: -d["box"][2])


def _clip_square(cx: float, cy: float, side: float, w: int, h: int) -> list[int]:
    x0, y0 = max(0, math.floor(cx - side / 2)), max(0, math.floor(cy - side / 2))
    x1, y1 = min(w, math.ceil(cx + side / 2)), min(h, math.ceil(cy + side / 2))
    return [x0, y0, x1 - x0, y1 - y0]


def seg_roi(landmarks_px: np.ndarray, w: int, h: int) -> list[int]:
    e1, e2 = landmarks_px[468, :2], landmarks_px[473, :2]
    iod = float(np.linalg.norm(e2 - e1))
    mid = (e1 + e2) / 2
    return _clip_square(mid[0], mid[1] + SEG_CENTRE_DOWN_IOD * iod, SEG_SIDE_IOD * iod, w, h)


def _landmarker_faces(rgb: np.ndarray, full_dets: list[dict], crops_only: bool = False) -> list[dict]:
    """Faces from the whole image, plus a crop around every full-range detection the whole-image run missed.

    ``crops_only`` (documents): skip the whole-image run. A card portrait is a small face (IOD ~50 px) in a large
    photo, where MediaPipe's whole-image run lands poorly (doc-menon: mouth corners ~10 px low, verified visually
    against the service, 2026-10-01); the crop route is more accurate and closer to the service's path.
    """
    h, w = rgb.shape[:2]
    faces = []
    res = mptasks.landmarks(rgb, NUM_FACES) if not crops_only else None
    for i, lms in enumerate(res.face_landmarks if res else []):
        pts = np.array([[p.x * w, p.y * h, p.z * w] for p in lms])
        faces.append(
            ("mediapipe-whole-image", None, pts, res.face_blendshapes[i], res.facial_transformation_matrixes[i])
        )

    def covered(box) -> bool:
        bx, by, bw, bh = box
        return any(bx <= f[2][1, 0] <= bx + bw and by <= f[2][1, 1] <= by + bh for f in faces)  # nose tip inside

    # Faces the whole-image run missed (its internal short-range detector misses small faces): one crop per
    # full-range detection, best first, keeping only the crop's largest face.
    for det in full_dets:
        if len(faces) >= NUM_FACES or covered(det["box"]):
            continue
        bx, by, bw, bh = det["box"]
        crop = _clip_square(bx + bw / 2, by + bh / 2, CROP_SIDE_BOX * bw, w, h)
        x0, y0, cw, ch = crop
        r2 = mptasks.landmarks(np.ascontiguousarray(rgb[y0 : y0 + ch, x0 : x0 + cw]), NUM_FACES)
        cands = [
            (np.array([[x0 + p.x * cw, y0 + p.y * ch, p.z * cw] for p in lms]), i)
            for i, lms in enumerate(r2.face_landmarks)
        ]
        cands = [c for c in cands if bx <= c[0][1, 0] <= bx + bw and by <= c[0][1, 1] <= by + bh]
        if cands:
            pts, i = max(cands, key=lambda c: np.linalg.norm(c[0][468, :2] - c[0][473, :2]))
            faces.append(
                ("mediapipe-full-range-crop", crop, pts, r2.face_blendshapes[i], r2.facial_transformation_matrixes[i])
            )

    def iod(f):
        return float(np.linalg.norm(f[2][468, :2] - f[2][473, :2]))

    return sorted(faces, key=lambda f: -iod(f))


def _document_upright_faces(rgb: np.ndarray, card_corners: list) -> list:
    """Document faces the way the service sees them: warp the card upright (ground-truth corners, native
    resolution), detect and landmark the portrait there (crop route), then map landmarks back to input pixels.

    A sideways or perspective-distorted portrait is landmarked worse in place (doc-watkins-rot90 read 3.4 deg of
    roll off the upright card; 2026-10-01). Ground truth exists only for fixtures, so the lab scorer for real
    documents keeps the crop route.
    """
    from facelab.testdata.documents import CARD_H, CARD_W

    c = np.float32(card_corners)
    k = document_frame_scale(card_corners)
    cw, ch = round(CARD_W * k), round(CARD_H * k)
    hmat = cv2.getPerspectiveTransform(c, np.float32([[0, 0], [cw, 0], [cw, ch], [0, ch]]))
    card = cv2.warpPerspective(rgb, hmat, (cw, ch), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
    inv = np.linalg.inv(hmat)
    faces = []
    for _source, _crop, pts, blend, matrix in _landmarker_faces(card, _detections(card, "full"), crops_only=True):
        xy = cv2.perspectiveTransform(pts[None, :, :2].astype(np.float64), inv)[0]
        m = pts[:, :2].mean(0)
        q = cv2.perspectiveTransform(np.array([[m, m + [1.0, 0.0], m + [0.0, 1.0]]], np.float64), inv)[0]
        scale = float(np.sqrt(abs(np.linalg.det(np.column_stack([q[1] - q[0], q[2] - q[0]])))))
        back = np.column_stack([xy, pts[:, 2] * scale])
        faces.append(("document-upright-crop", None, back, blend, matrix))
    return faces


def document_frame_scale(card_corners: list) -> float:
    """Rectified card px per ID-1 card px: the service's no-upscaling rule (architecture 3.2), i.e. the shorter of
    the two horizontal edges over the card width and the shorter vertical edge over the card height, whichever is
    smaller. Matches the service's document INTER_EYE_DISTANCE within 0.4% on the perspective fixture."""
    from facelab.testdata.documents import CARD_H, CARD_W

    c = np.asarray(card_corners, np.float64)
    top, right, bottom, left = (float(np.linalg.norm(c[(i + 1) % 4] - c[i])) for i in range(4))
    return min(min(top, bottom) / CARD_W, min(left, right) / CARD_H)


def to_document_frame(pts: np.ndarray, card_corners: list) -> np.ndarray:
    """Landmarks (x, y, z in input px) -> the rectified, upright card frame (contract: a document portrait's pose
    is measured there). The homography maps the ground-truth card corners (TL, TR, BR, BL of the upright card) to
    the ID-1 card rectangle; z is scaled by the homography's local scale at the face (sqrt |det J| at the
    landmark centroid), so it stays in the same units as x and y."""
    from facelab.testdata.documents import CARD_H, CARD_W

    dst = np.float32([[0, 0], [CARD_W, 0], [CARD_W, CARD_H], [0, CARD_H]])
    hmat = cv2.getPerspectiveTransform(np.float32(card_corners), dst)
    xy = cv2.perspectiveTransform(pts[None, :, :2].astype(np.float64), hmat)[0]
    c = pts[:, :2].mean(0)
    probe = np.array([[c, c + [1.0, 0.0], c + [0.0, 1.0]]], np.float64)
    q = cv2.perspectiveTransform(probe, hmat)[0]
    jac = np.column_stack([q[1] - q[0], q[2] - q[0]])
    scale = float(np.sqrt(abs(np.linalg.det(jac))))
    return np.column_stack([xy, pts[:, 2] * scale])


def _doc_iod(pts: np.ndarray, card_corners: list) -> float:
    """IOD on the rectified, upright card at the service's rectification scale (document INTER_EYE_DISTANCE)."""
    d = to_document_frame(pts, card_corners) * document_frame_scale(card_corners)
    return round(float(np.linalg.norm(d[468, :2] - d[473, :2])), 3)


def _face_json(rgb: np.ndarray, image_id: str, k: int, face, card_corners: list | None = None) -> tuple[dict, dict]:
    source, crop, pts, blend, matrix = face
    h, w = rgb.shape[:2]
    m = np.array(matrix, np.float64)
    input_fit = pose.fit_weak_perspective(pts)
    # Documents: pose in the upright card frame (contract, lead decision 2026-10-01); input-frame pose kept as info.
    fit = pose.fit_weak_perspective(to_document_frame(pts, card_corners)) if card_corners else input_fit
    a, b, c = pose.euler_zyx(fit["rotation"])
    p5 = pts[list(align.LANDMARKS5), :2]
    aligned, amat = align.aligned_crop(rgb, p5)
    raw, emb = align.embed(aligned)
    out = {
        "source": source,
        "crop": crop,
        "iod": round(float(np.linalg.norm(pts[468, :2] - pts[473, :2])), 3),
        "iodDocumentFrame": _doc_iod(pts, card_corners) if card_corners else None,
        "landmarks": _r(pts, 3),
        "blendshapes": {cat.category_name: round(cat.score, 6) for cat in blend},
        "pose": {key: round(fit[key], 4) for key in ("yaw", "pitch", "roll")},
        "poseFrame": "document" if card_corners else "input",
        "poseInputFrame": {key: round(input_fit[key], 4) for key in ("yaw", "pitch", "roll")},
        "poseFit": {
            "rotation": _r(fit["rotation"], 6),
            "scalePxPerCm": round(fit["scale"], 4),
            "weightedRmsResidualPx": round(fit["weightedRmsResidualPx"], 3),
        },
        "eulerRaw": {"a": round(a, 4), "b": round(b, 4), "c": round(c, 4)},
        "poseMediapipeMatrix": {key: round(v, 4) for key, v in pose.pose_from_matrix(m).items()},
        "transformMatrix": _r(m, 6),
        "landmarks5": _r(p5, 3),
        "alignment": {"matrix": _r(amat, 6), "template": align.TEMPLATE_112.tolist()},
        "embedding": _r(emb, 6),
        "segmentation": None,
        "illumination": None,
    }
    extras = {"aligned": aligned, "embeddingRaw": raw}
    if k == 0:
        x0, y0, sw, sh = seg_roi(pts, w, h)
        sub = np.ascontiguousarray(rgb[y0 : y0 + sh, x0 : x0 + sw])
        seg = mptasks.segment(sub)
        cat = seg.category_mask.numpy_view().copy()
        if cat.ndim == 3:  # MediaPipe returns (H, W, 1)
            cat = cat[:, :, 0]
        mask_name = f"masks/{image_id}.png"
        _save_png(GOLDEN_DIR / mask_name, cat.astype(np.uint8))
        fx0, fy0 = pts[:, 0].min() - x0, pts[:, 1].min() - y0
        fx1, fy1 = pts[:, 0].max() - x0, pts[:, 1].max() - y0
        face_region = cat[
            max(0, int(fy0)) : max(0, int(math.ceil(fy1))), max(0, int(fx0)) : max(0, int(math.ceil(fx1)))
        ]

        def fractions(arr):
            n = max(arr.size, 1)
            return {name: round(float((arr == i).sum()) / n, 5) for i, name in enumerate(SEG_CLASSES)}

        out["segmentation"] = {
            "roi": [x0, y0, sw, sh],
            "maskFile": mask_name,
            "classFractions": fractions(cat),
            "faceBoxFractions": fractions(face_region),
        }
        region = measures.face_skin_region(pts, cat, [x0, y0, sw, sh], (h, w))
        out["illumination"] = measures.illumination(rgb, region)
        extras["segRoi"] = [x0, y0, sw, sh]
    return out, extras


def _check_invariants(golden: dict, parent: dict, transform: str) -> dict:
    inv = dict(INVARIANTS[transform])
    tol = inv["toleranceDeg"]
    gp, pp = golden["faces"][0]["pose"], parent["faces"][0]["pose"]
    expect = {
        "yaw": -pp["yaw"] if inv["yaw"] == "negated" else pp["yaw"],
        "pitch": pp["pitch"],
        "roll": -pp["roll"] if inv["roll"] == "negated" else pp["roll"] + (15.0 if inv["roll"] == "plus 15" else 0.0),
    }
    for key, want in expect.items():
        if abs(gp[key] - want) > tol:
            raise GoldenError(f"{golden['image']['id']}: invariant {key} expected {want:.2f}, got {gp[key]:.2f}")
    return {"comparedWith": parent["image"]["id"], "transform": transform, **inv}


def _model_level(image_id: str, rgb: np.ndarray, faces_extras: list[dict], primary: dict | None) -> None:
    out_dir = GOLDEN_DIR / "model" / image_id
    out_dir.mkdir(parents=True, exist_ok=True)
    index = {"image": image_id, "tolerances": MODEL_TOLERANCES, "models": {}}

    def save_npy(name: str, arr: np.ndarray, fp16: bool = False) -> str:
        np.save(out_dir / name, arr.astype(np.float16 if fp16 else np.float32))
        return name

    dets_by_variant = {}
    for variant, model_id in pipeline.DETECTOR_IDS.items():
        dets, io = pipeline.detect(rgb, variant)
        dets_by_variant[variant] = dets
        _save_png(out_dir / f"{model_id}.input.png", io["tensorImage"])
        index["models"][model_id] = {
            "input": f"{model_id}.input.png",
            "roi": io["roi"].to_json(),
            "keepAspectRatio": True,
            "padding": list(io["padding"]),
            "border": "zero",
            "warpMatrix": _r(io["matrix"], 8),
            "outputs": {
                "regressors": save_npy(f"{model_id}.regressors.npy", io["regressors"]),
                "classificators": save_npy(f"{model_id}.classificators.npy", io["classificators"]),
            },
            "decoded": [d.to_json() for d in dets],
        }
    source_dets = dets_by_variant["short"] or dets_by_variant["full"]
    if source_dets:
        det = max(source_dets, key=lambda d: d.box[2])
        roi = blazeface.landmark_roi(det)
        lm_px, presence, io = pipeline.landmarks(rgb, roi)
        _save_png(out_dir / "mp-face-landmarker.input.png", io["tensorImage"])
        index["models"]["mp-face-landmarker"] = {
            "input": "mp-face-landmarker.input.png",
            "roiFromDetector": "short" if dets_by_variant["short"] else "full",
            "roi": roi.to_json(),
            "keepAspectRatio": False,
            "border": "replicate",
            "warpMatrix": _r(io["matrix"], 8),
            "outputs": {
                k: save_npy(f"mp-face-landmarker.{k}.npy", io[k]) for k in ("landmarks", "presence_logit", "tongue_out")
            },
            "presence": round(presence, 6),
        }
        pts = lm_px[LANDMARK_SUBSET, :2][None].astype(np.float32)
        blend = pipeline.session("mp-face-blendshapes").run(None, {"points": pts})[0]
        index["models"]["mp-face-blendshapes"] = {
            "input": save_npy("mp-face-blendshapes.points.npy", pts),
            "inputNote": "146-landmark subset (x, y) in full-image px, from the mp-face-landmarker golden above",
            "outputs": {"blendshapes": save_npy("mp-face-blendshapes.blendshapes.npy", blend)},
        }
    if primary is not None and "segRoi" in primary:
        x0, y0, sw, sh = primary["segRoi"]
        roi = RotatedRect(x0 + sw / 2, y0 + sh / 2, float(sw), float(sh), 0.0)
        img, m = warp(rgb, roi, 256, 256, border="replicate")
        logits = pipeline.session("mp-selfie-multiclass").run(None, {"input": normalise(img, "(x")})[0]
        _save_png(out_dir / "mp-selfie-multiclass.input.png", img)
        index["models"]["mp-selfie-multiclass"] = {
            "input": "mp-selfie-multiclass.input.png",
            "roi": roi.to_json(),
            "keepAspectRatio": False,
            "border": "replicate",
            "warpMatrix": _r(m, 8),
            "outputs": {"logits": save_npy("mp-selfie-multiclass.logits.f16.npy", logits, fp16=True)},
            "outputNote": "stored as float16; compare with atol 2e-2 (float16 rounding of logits up to ~20)",
        }
    if primary is not None:
        _save_png(out_dir / "auraface-v1.input.png", primary["aligned"])
        index["models"]["auraface-v1"] = {
            "input": "auraface-v1.input.png",
            "inputNote": "aligned 112x112 crop of the primary face (pipeline golden 'alignment')",
            "outputs": {"embedding": save_npy("auraface-v1.embedding.npy", primary["embeddingRaw"][None])},
        }
    (out_dir / "index.json").write_text(json.dumps(index, indent=1) + "\n", encoding="utf-8", newline="\n")


def build(ids: list[str] | None = None) -> None:
    images = provenance.all_images()
    if ids:
        unknown = [i for i in ids if i not in images]
        if unknown:
            raise GoldenError(f"not in testdata/provenance.json (only PD/synthetic fixtures allowed): {unknown}")
    order = ids or list(images)
    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    models = {m["id"]: m["sha256"] for m in manifest["models"]}
    generator = {
        "facelab": __version__,
        "mediapipe": version("mediapipe"),
        "onnxruntime": version("onnxruntime"),
        "opencv": cv2.__version__,
        "numpy": np.__version__,
        "models": models,
    }
    (GOLDEN_DIR / "pipeline").mkdir(parents=True, exist_ok=True)
    done: dict[str, dict] = {}
    # Parents before derived fixtures so invariants can be checked.
    order = sorted(order, key=lambda i: "parent" in images[i])
    for image_id in order:
        row = images[image_id]
        rgb = _load(row)
        h, w = rgb.shape[:2]
        full = _detections(rgb, "full")
        golden = {
            "formatVersion": FORMAT_VERSION,
            "image": {"id": image_id, "file": row["file"], "sha256": row["sha256"], "width": w, "height": h},
            "generator": generator,
            "detections": {"short": _detections(rgb, "short"), "full": full, "sources": DETECTION_SOURCES},
            "faces": [],
        }
        extras_all = []
        if "groundTruth" in row and row["groundTruth"]["mainPortraits"] >= 1:
            faces_found = _document_upright_faces(rgb, row["groundTruth"]["cardCorners"])
        else:
            faces_found = _landmarker_faces(rgb, full, crops_only="groundTruth" in row)
        for k, face in enumerate(faces_found):
            card = row["groundTruth"]["cardCorners"] if "groundTruth" in row else None
            fj, extras = _face_json(rgb, image_id, k, face, card)
            golden["faces"].append(fj)
            extras_all.append(extras)
        if "groundTruth" in row:
            golden["documents"] = row["groundTruth"]
        ref_id = row.get("compareTo", row.get("parent"))
        if ref_id:
            parent = done.get(ref_id) or json.loads(
                (GOLDEN_DIR / "pipeline" / f"{ref_id}.json").read_text(encoding="utf-8")
            )
            golden["invariants"] = _check_invariants(golden, parent, row["transform"])
        golden["tolerances"] = TOLERANCES
        path = GOLDEN_DIR / "pipeline" / f"{image_id}.json"
        path.write_text(json.dumps(golden, separators=(",", ":")) + "\n", encoding="utf-8", newline="\n")
        done[image_id] = golden
        if image_id in MODEL_LEVEL_IDS:
            _model_level(image_id, rgb, extras_all, extras_all[0] if extras_all else None)
        nf = len(golden["faces"])
        p = golden["faces"][0]["pose"] if nf else {}
        pose_s = " ".join(f"{k} {v:+6.1f}" for k, v in p.items())
        print(f"ok {image_id:28s} det s/f {len(golden['detections']['short'])}/{len(full)} faces {nf} {pose_s}")
