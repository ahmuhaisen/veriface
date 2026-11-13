"""``facelab testdata documents``: synthetic ID-1 "SPECIMEN" cards built from the PD portraits.

The layout is our own invention: no real issuer's design, emblem, security feature or number format. Every
document is photographed onto a synthetic tabletop so the rectifier has a card edge to find, and its ground
truth (card corners, rotation, portrait boxes) is stored in testdata/provenance.json for the Rust tests.

Deterministic: fixed seeds, Pillow's bundled font, fixed JPEG settings.
"""

from __future__ import annotations

import hashlib
import io
import math
from dataclasses import dataclass

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageFont

from facelab.paths import DOCUMENTS_DIR, IMAGES_DIR
from facelab.testdata import provenance

# ID-1 is 85.60 x 53.98 mm; at 300 dpi that is 1011 x 638 px.
CARD_W, CARD_H = 1011, 638
CORNER_R = 35
PORTRAIT = (40, 120, 270, 360)  # x, y, w, h in card px (3:4)
GHOST = (835, 330, 120, 160)
TWIN = (330, 120, 270, 360)  # second main portrait for the two-faces case
SCENE_W, SCENE_H = 1600, 1200
JPEG_QUALITY = 90

FIELDS = [
    ("SURNAME", "SPECIMEN"),
    ("GIVEN NAMES", "TEST SAMPLE"),
    ("DATE OF BIRTH", "01 JAN 1990"),
    ("DOCUMENT NO.", "TEST-000-0000"),
    ("EXPIRY", "31 DEC 2099"),
]


@dataclass(frozen=True)
class Doc:
    id: str
    portrait: str | None  # source portrait id (None = no face)
    variant: str
    twin: str | None = None
    rotate: int = 0  # 0/90/180/270, clockwise, applied to the whole photo
    perspective: bool = False
    glare: bool = False
    blur: bool = False
    lowres: bool = False
    jpeg_quality: int = JPEG_QUALITY
    scan: bool = False  # flat scan of the card only, no tabletop


DOCS: list[Doc] = [
    Doc("doc-watkins", "nasa-watkins", "upright"),
    Doc("doc-almansoori", "nasa-almansoori", "upright"),
    Doc("doc-menon", "nasa-menon", "upright"),
    Doc("doc-ochoa-scan", "nasa-ochoa", "flat scan, card only", scan=True),
    Doc("doc-watkins-rot90", "nasa-watkins", "rotated 90 cw", rotate=90),
    Doc("doc-watkins-rot180", "nasa-watkins", "rotated 180", rotate=180),
    Doc("doc-watkins-rot270", "nasa-watkins", "rotated 270 cw", rotate=270),
    Doc("doc-almansoori-perspective", "nasa-almansoori", "perspective warp", perspective=True),
    Doc("doc-watkins-glare", "nasa-watkins", "glare spot over portrait", glare=True),
    Doc("doc-menon-blur", "nasa-menon", "gaussian blur sigma 4", blur=True),
    Doc("doc-almansoori-lowres", "nasa-almansoori", "low resolution (400x300)", lowres=True),
    Doc("doc-watkins-jpeg", "nasa-watkins", "heavy JPEG (quality 8)", jpeg_quality=8),
    Doc("doc-noface", None, "no face (photo area blank)"),
    Doc("doc-twofaces", "nasa-chari", "two equal main portraits", twin="nasa-ochoa"),
]


def _font(size: int) -> ImageFont.FreeTypeFont:
    return ImageFont.load_default(size=size)


def _load(pid: str) -> np.ndarray:
    return cv2.cvtColor(cv2.imread(str(IMAGES_DIR / f"{pid}.jpg")), cv2.COLOR_BGR2RGB)


def portrait_crop(pid: str) -> Image.Image:
    """3:4 head-and-shoulders crop around the largest detected face (full-range).

    The committed documents (2026-10-01) were built with MediaPipe 1.0.1 Tasks' integer full-range boxes; the
    telemetry-free 0.10.21 cannot run full-range, so rebuilds use the lab reference detector (float boxes) and
    the crops may move by a pixel. Rebuild only when the documents must change anyway.
    """
    from facelab.ref import pipeline

    rgb = _load(pid)
    dets = pipeline.detect(rgb, "full")[0]
    if not dets:
        raise RuntimeError(f"{pid}: no face found for the card portrait")
    bx, by, bw, bh = max(dets, key=lambda d: d.box[2]).box
    cx, cy = bx + bw / 2, by + bh * 0.45
    w = bw * 2.0
    h = w * 4 / 3
    x0, y0 = max(0, cx - w / 2), max(0, cy - h * 0.45)
    x1, y1 = min(rgb.shape[1], x0 + w), min(rgb.shape[0], y0 + h)
    crop = Image.fromarray(rgb[int(y0) : int(y1), int(x0) : int(x1)])
    return crop.resize((PORTRAIT[2], PORTRAIT[3]), Image.Resampling.LANCZOS)


def _guilloche(rng: np.random.Generator) -> Image.Image:
    yy, xx = np.mgrid[0:CARD_H, 0:CARD_W].astype(np.float32)
    base = np.stack(
        [
            225 + 15 * np.sin(xx / 37.0 + 3 * np.sin(yy / 53.0)),
            232 + 12 * np.sin(yy / 29.0 + 2 * np.sin(xx / 71.0)),
            215 + 18 * np.cos((xx + yy) / 45.0),
        ],
        axis=-1,
    )
    base += rng.normal(0, 2, base.shape)
    return Image.fromarray(np.clip(base, 0, 255).astype(np.uint8))


def render_card(doc: Doc, rng: np.random.Generator) -> tuple[Image.Image, dict]:
    card = _guilloche(rng).convert("RGBA")
    d = ImageDraw.Draw(card)
    d.rectangle([0, 0, CARD_W, 90], fill=(40, 70, 120, 255))
    d.text((40, 18), "SPECIMEN IDENTITY CARD", font=_font(40), fill="white")
    d.text((40, 62), "NOT A VALID DOCUMENT - VERIFACE SYNTHETIC TEST DATA", font=_font(18), fill=(220, 230, 255))
    truth: dict = {"mainPortraits": [], "ghostPortraits": []}

    def paste(img: Image.Image, box: tuple[int, int, int, int], key: str, alpha: float = 1.0) -> None:
        x, y, w, h = box
        tile = img.resize((w, h), Image.Resampling.LANCZOS).convert("RGBA")
        if alpha < 1.0:
            tile.putalpha(int(255 * alpha))
        card.alpha_composite(tile, (x, y))
        truth[key].append([x, y, w, h])

    fx = PORTRAIT[0] + PORTRAIT[2] + 30
    if doc.portrait is None:
        x, y, w, h = PORTRAIT
        d.rectangle([x, y, x + w, y + h], fill=(200, 200, 200, 255), outline=(120, 120, 120, 255), width=3)
        d.text((x + 70, y + 165), "NO PHOTO", font=_font(28), fill=(90, 90, 90))
    else:
        face = portrait_crop(doc.portrait)
        paste(face, PORTRAIT, "mainPortraits")
        if doc.twin:
            paste(portrait_crop(doc.twin), TWIN, "mainPortraits")
            fx = TWIN[0] + TWIN[2] + 30
        else:
            paste(face.convert("L").convert("RGB"), GHOST, "ghostPortraits", alpha=0.45)
    y = 125
    for label, value in FIELDS:
        d.text((fx, y), label, font=_font(16), fill=(70, 70, 90))
        d.text((fx, y + 20), value, font=_font(28), fill=(20, 20, 30))
        y += 70
    d.text((40, CARD_H - 50), "P<SPECIMEN<<TEST<SAMPLE<<<<<<<<<<<<<<<<<<<<", font=_font(26), fill=(30, 30, 30))

    stamp = Image.new("RGBA", (CARD_W, CARD_H), (0, 0, 0, 0))
    ImageDraw.Draw(stamp).text((180, 260), "SPECIMEN", font=_font(150), fill=(200, 30, 30, 70))
    card.alpha_composite(stamp.rotate(18, resample=Image.Resampling.BICUBIC, center=(CARD_W / 2, CARD_H / 2)))

    mask = Image.new("L", (CARD_W, CARD_H), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, CARD_W - 1, CARD_H - 1], CORNER_R, fill=255)
    card.putalpha(mask)
    return card, truth


def _tabletop(rng: np.random.Generator) -> np.ndarray:
    yy, xx = np.mgrid[0:SCENE_H, 0:SCENE_W].astype(np.float32)
    grain = 12 * np.sin(xx / 9.0 + 4 * np.sin(yy / 140.0)) + rng.normal(0, 6, (SCENE_H, SCENE_W))
    shade = 30 * (xx / SCENE_W) + 20 * (yy / SCENE_H)
    r, g, b = 120 + grain + shade, 85 + grain * 0.8 + shade, 55 + grain * 0.6 + shade
    return np.clip(np.stack([r, g, b], -1), 0, 255).astype(np.uint8)


def _place(card: Image.Image, doc: Doc, rng: np.random.Generator) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Composite the card onto the tabletop. Returns (scene, card->scene homography, card corners in scene)."""
    src = np.float32([[0, 0], [CARD_W, 0], [CARD_W, CARD_H], [0, CARD_H]])
    scale = 1050 / CARD_W
    angle = math.radians(rng.uniform(-6, 6))
    c, s = math.cos(angle), math.sin(angle)
    cx, cy = SCENE_W / 2 + rng.uniform(-60, 60), SCENE_H / 2 + rng.uniform(-40, 40)
    dst = np.float32(
        [
            [
                cx + scale * ((x - CARD_W / 2) * c - (y - CARD_H / 2) * s),
                cy + scale * ((x - CARD_W / 2) * s + (y - CARD_H / 2) * c),
            ]
            for x, y in src
        ]
    )
    if doc.perspective:
        dst += np.float32([[60, 40], [-30, 70], [-90, -20], [40, -60]])  # strong keystone, still convex
    hmat = cv2.getPerspectiveTransform(src, dst)
    rgba = np.asarray(card)
    warped = cv2.warpPerspective(rgba, hmat, (SCENE_W, SCENE_H), flags=cv2.INTER_LINEAR, borderValue=(0, 0, 0, 0))
    a = warped[..., 3:4].astype(np.float32) / 255.0
    scene = _tabletop(rng).astype(np.float32)
    shadow = cv2.GaussianBlur(a[..., 0], (0, 0), 12)[..., None]
    shadow = np.roll(shadow, (14, 10), axis=(0, 1))
    scene *= 1 - 0.35 * shadow
    scene = scene * (1 - a) + warped[..., :3].astype(np.float32) * a
    return np.clip(scene, 0, 255).astype(np.uint8), hmat, dst


def _glare(img: np.ndarray, center: tuple[float, float], radius: float) -> np.ndarray:
    h, w = img.shape[:2]
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    d2 = ((xx - center[0]) / radius) ** 2 + ((yy - center[1]) / (radius * 0.6)) ** 2
    spot = 255 * np.exp(-d2 * 2.0)[..., None]
    return np.clip(img.astype(np.float32) + spot, 0, 255).astype(np.uint8)


def _project(hmat: np.ndarray, boxes: list[list[int]]) -> list[list[list[float]]]:
    out = []
    for x, y, w, h in boxes:
        pts = np.float32([[[x, y], [x + w, y], [x + w, y + h], [x, y + h]]])
        out.append(np.round(cv2.perspectiveTransform(pts, hmat)[0], 1).tolist())
    return out


def _rotate_points(pts: np.ndarray, rot: int, w: int, h: int) -> np.ndarray:
    """Map (x, y) through a clockwise rotation of a w x h image by ``rot`` degrees."""
    x, y = pts[..., 0], pts[..., 1]
    if rot == 90:
        return np.stack([h - 1 - y, x], -1)
    if rot == 180:
        return np.stack([w - 1 - x, h - 1 - y], -1)
    if rot == 270:
        return np.stack([y, w - 1 - x], -1)
    return pts


def _encode(rgb: np.ndarray, quality: int) -> bytes:
    buf = io.BytesIO()
    Image.fromarray(rgb).save(buf, "JPEG", quality=quality, optimize=False, progressive=False)
    return buf.getvalue()


def make(doc: Doc, seed: int) -> tuple[bytes, dict]:
    rng = np.random.default_rng(seed)
    card, truth = render_card(doc, rng)
    expected: dict = {
        "mainPortraits": len(truth["mainPortraits"]),
        "ghostPortraits": len(truth["ghostPortraits"]),
        "rotationCw": doc.rotate,
    }
    if doc.scan:
        flat = Image.new("RGB", card.size, (255, 255, 255))
        flat.paste(card, (0, 0), card)
        img = np.asarray(flat)
        hmat = np.eye(3)
        corners = np.float32([[0, 0], [CARD_W, 0], [CARD_W, CARD_H], [0, CARD_H]])
    else:
        img, hmat, corners = _place(card, doc, rng)
    portraits = _project(hmat, truth["mainPortraits"])
    ghosts = _project(hmat, truth["ghostPortraits"])
    if doc.glare and portraits:
        q = np.float32(portraits[0])
        # Partial glare: centred on the portrait's upper-right corner, so it washes out part of the face.
        img = _glare(img, (float(q[1, 0]), float(q[1, 1] + 60)), 110)
    if doc.blur:
        img = cv2.GaussianBlur(img, (0, 0), 4)
    if doc.lowres:
        factor = 400 / img.shape[1]
        img = cv2.resize(img, (400, round(img.shape[0] * factor)), interpolation=cv2.INTER_AREA)
        corners = corners * factor
        portraits = (np.float32(portraits) * factor).round(1).tolist() if portraits else []
        ghosts = (np.float32(ghosts) * factor).round(1).tolist() if ghosts else []
    if doc.rotate:
        h, w = img.shape[:2]
        img = np.ascontiguousarray(np.rot90(img, k=-doc.rotate // 90))
        corners = _rotate_points(np.float32(corners), doc.rotate, w, h)
        portraits = [_rotate_points(np.float32(p), doc.rotate, w, h).round(1).tolist() for p in portraits]
        ghosts = [_rotate_points(np.float32(p), doc.rotate, w, h).round(1).tolist() for p in ghosts]
    expected["cardCorners"] = np.round(np.float32(corners), 1).tolist()  # card TL, TR, BR, BL in image px
    expected["mainPortraitQuads"] = portraits
    expected["ghostPortraitQuads"] = ghosts
    return _encode(img, doc.jpeg_quality), {"size": f"{img.shape[1]}x{img.shape[0]}", "expected": expected}


def build() -> list[dict]:
    DOCUMENTS_DIR.mkdir(parents=True, exist_ok=True)
    rows = []
    for i, doc in enumerate(DOCS):
        data, info = make(doc, seed=1000 + i)
        path = DOCUMENTS_DIR / f"{doc.id}.jpg"
        path.write_bytes(data)
        e = info["expected"]
        summary = f"main {e['mainPortraits']}, ghost {e['ghostPortraits']}, rotation {e['rotationCw']}"
        rows.append(
            {
                "id": doc.id,
                "file": f"documents/{path.name}",
                "size": info["size"],
                "sha256": hashlib.sha256(data).hexdigest(),
                "sources": [p for p in (doc.portrait, doc.twin) if p],
                "variant": doc.variant,
                "expected": summary,
                "groundTruth": e,
            }
        )
        print(f"ok {doc.id:28s} {info['size']:>9s}  {summary}")
    data = provenance.load()
    data["documents"] = rows
    provenance.save(data)
    return rows
