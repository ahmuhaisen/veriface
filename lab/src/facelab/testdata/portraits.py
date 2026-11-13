"""``facelab testdata portraits``: public-domain NASA portraits from Wikimedia Commons.

Each image is accepted only if (a) its Commons page's wikitext carries ``{{PD-USGov-NASA}}`` (a work of a
NASA employee, public domain in the US under 17 U.S.C. 105) and (b) the original file matches the SHA-1 that
Commons reported when we curated it. Originals are cached in ``lab/work/portraits/`` (git-ignored); the
committed images are downscaled to a long side of 1024 px with all metadata dropped.

Note on use: public domain covers copyright only. NASA's media guidelines ask that images of identifiable
astronauts not be used to imply endorsement; these files are test fixtures and are never published.
"""

from __future__ import annotations

import hashlib
import json
import re
import shutil
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageOps

from facelab.paths import IMAGES_DIR, WORK_DIR
from facelab.testdata import provenance

API = "https://commons.wikimedia.org/w/api.php"
INDEX = "https://commons.wikimedia.org/w/index.php"
UA = {"User-Agent": "facelab/0.1 (Veriface test-data curation; https://github.com/ahmuhaisen/veriface)"}
LONG_SIDE = 1024
JPEG_QUALITY = 92
PD_TEMPLATE = re.compile(r"\{\{\s*PD-USGov-NASA\s*(\|[^}]*)?\}\}", re.I)


@dataclass(frozen=True)
class Portrait:
    id: str
    title: str  # Commons file title without the "File:" prefix
    sha1: str  # SHA-1 of the original as reported by the Commons API at curation time (2026-10-01)
    note: str = ""


PORTRAITS: list[Portrait] = [
    Portrait("nasa-glover", "Victor J. Glover official portrait.jpg", "704fc3f12fde82adb3bcd29b66b43e725382ded7"),
    Portrait("nasa-watkins", "Jessica Watkins official portrait.jpg", "459463a92a15f4a243c4edb20aafb7ff69af6574"),
    Portrait("nasa-chari", "Raja Chari portrait.jpg", "20dbe4257f574a307aaaa0c58f9a4d1e567e84c7"),
    Portrait("nasa-menon", "Anil Menon portrait.jpg", "08bd951d2b67620f4e8c20741b96f7bbf6c890aa"),
    Portrait("nasa-moghbeli", "Jasmin Moghbeli official portrait.jpg", "69a05dd33b1724824c2a0f1e8484fcadd17e0b39"),
    Portrait("nasa-almansoori", "Hazzaa AlMansoori.jpg", "fff8b30845d588c5efff80aa9abe1926ce6835df"),
    Portrait("nasa-rubio", "Frank Rubio official portrait.jpg", "89b5f2667652e0a378e8870634a0e7f62aa5a992"),
    Portrait("nasa-ochoa", "Ellen Ochoa, official portrait.jpg", "8f3ce8d0769f93c14d514967ee5ee7f0db94911a"),
    Portrait(
        "nasa-hoshide", "Akihiko Hoshide, official portrait (2020).jpg", "48bade0ffbb1e1e7c5e9bbce176cb6db5e0e39e9"
    ),
    Portrait("nasa-cardman", "Zena Cardman portrait.jpg", "db5ce17898a76a044e60dcecdbcac27ce46c84fe"),
    Portrait("nasa-hammock", "Christina M. Hammock portrait.jpg", "8d70b051fa71fa4282a96af64cd3250e5979a073"),
    Portrait("nasa-cassada", "Josh A. Cassada official portrait.jpg", "de102f0dc66bfd20c7b4842e2997b7607031fb7d"),
    Portrait("nasa-hoburg", "Warren Hoburg portrait.jpg", "99cb4ab0d1b1c634b7a5d4d892b43c1bcd3e03bb"),
    Portrait("nasa-hansen", "Jeremy Hansen official portrait.jpg", "f2973af23d2bcfd7bb872defb186ed6e7be363cd"),
    Portrait("nasa-harris", "Bernard A. Harris - Official Portrait.jpg", "a8f4fc2e8ca79d05ec47193e8a8e2784b3d295fe"),
    Portrait("nasa-isaacman", "Jared isaacman official portrait.jpg", "ecce61b429447f5e9e19c87b630bb7c938b37852"),
    # Replaced nasa-blackwell-thompson (2026-10-01): its nasa.gov thumbnail had "See Rights Usage terms" burned
    # into the pixels, a sign it passed through a rights-managed system despite the PD tag. Never re-add it.
    Portrait(
        "nasa-caldwell",
        "Tracy E Caldwell portrait.jpg",
        "88b1f7da42c021be891a7ace23c6ed45187d6976",
        "NASA JSC2007-E-033311",
    ),
    Portrait(
        "nasa-lovell-1964",
        "Jim Lovell official 1964 portrait.jpg",
        "87e2c068825f04b04392ed73b7b5ec9bdb0664f0",
        "1964 colour film scan",
    ),
    Portrait("nasa-dominick", "Matthew Dominick portrait.jpg", "9dba6e1fa1baa1572fb098c19e131d9b2b00e500"),
    Portrait("nasa-newman", "Dava Newman, official portrait.jpg", "7f75dbd4ee20f412d5fb11f6e6250e132dbe8359"),
    Portrait("nasa-meir", "Jessica Meir (portrait crop).jpg", "a03b3b36ee26d5b048c328d38ad0e7530a67b11b"),
    Portrait(
        "nasa-exp49-crew",
        "Expedition 49 crew portrait with astronaut Kate Rubins, cosmonaut Anatoly Ivanishin, and astronaut "
        "Takuya Onishi.jpg",
        "4f0e865a65d97ce127e4cd6ef090582bb6eda6be",
        "group photo, three faces (multi-face case)",
    ),
]


def _get(url: str) -> bytes:
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=120) as r:
        return r.read()


def _imageinfo(title: str) -> dict:
    q = {"action": "query", "prop": "imageinfo", "iiprop": "url|sha1|size", "titles": "File:" + title}
    d = json.loads(_get(API + "?" + urllib.parse.urlencode({**q, "format": "json"})))
    page = next(iter(d["query"]["pages"].values()))
    return {**page["imageinfo"][0]}


def _wikitext(title: str) -> str:
    return _get(INDEX + "?" + urllib.parse.urlencode({"title": "File:" + title, "action": "raw"})).decode("utf-8")


def _author(wikitext: str) -> str:
    m = re.search(r"\|\s*author\s*=\s*(.+)", wikitext, re.I)
    if not m:
        return "NASA"
    a = m.group(1).strip()
    a = re.sub(r"\{\{\s*Creator:([^}|]+)\}\}", r"\1", a)
    a = re.sub(r"\[\[(?:[^|\]]*\|)?([^\]]+)\]\]", r"\1", a)
    a = re.sub(r"\[\S+\s+([^\]]+)\]", r"\1", a)
    return re.sub(r"[{}]", "", a).strip() or "NASA"


def downscale(src: Path, dest: Path) -> tuple[int, int]:
    with Image.open(src) as im:
        im = ImageOps.exif_transpose(im).convert("RGB")
        w, h = im.size
        s = LONG_SIDE / max(w, h)
        if s < 1:
            im = im.resize((round(w * s), round(h * s)), Image.Resampling.LANCZOS)
        dest.parent.mkdir(parents=True, exist_ok=True)
        im.save(dest, "JPEG", quality=JPEG_QUALITY, optimize=False, progressive=False)  # no EXIF/ICC written
        return im.size


def build() -> list[dict]:
    cache = WORK_DIR / "portraits"
    cache.mkdir(parents=True, exist_ok=True)
    rows = []
    for p in PORTRAITS:
        wt = _wikitext(p.title)
        if not PD_TEMPLATE.search(wt):
            raise RuntimeError(f"{p.id}: Commons page no longer carries {{{{PD-USGov-NASA}}}}; refusing it")
        info = _imageinfo(p.title)
        if info["sha1"] != p.sha1:
            raise RuntimeError(f"{p.id}: Commons original changed (sha1 {info['sha1']} != pinned {p.sha1})")
        orig = cache / f"{p.id}{Path(p.title).suffix.lower()}"
        if not orig.exists() or hashlib.sha1(orig.read_bytes()).hexdigest() != p.sha1:
            tmp = orig.with_suffix(".part")
            tmp.write_bytes(_get(info["url"]))
            if hashlib.sha1(tmp.read_bytes()).hexdigest() != p.sha1:
                raise RuntimeError(f"{p.id}: downloaded file does not match sha1 {p.sha1}")
            shutil.move(tmp, orig)
        dest = IMAGES_DIR / f"{p.id}.jpg"
        w, h = downscale(orig, dest)
        rows.append(
            {
                "id": p.id,
                "file": f"images/{dest.name}",
                "size": f"{w}x{h}",
                "sha256": hashlib.sha256(dest.read_bytes()).hexdigest(),
                "page": info["descriptionurl"],
                "original": info["url"],
                "originalSha1": p.sha1,
                "author": _author(wt),
                "note": p.note,
            }
        )
        print(f"ok {p.id:26s} {w}x{h}")
    data = provenance.load()
    data["portraits"] = rows
    provenance.save(data)
    build_derived()
    return rows


# Pose fixtures (testdata/golden/README.md): (id, source image, transform, golden to compare with or None).
# canvas-*: the source pasted at the top / bottom of a mid-grey canvas twice its height (position invariance);
# the bottom placement is compared with the top one (same landmarker input scale).
DERIVED = [
    ("nasa-glover-flip", "nasa-glover", "flip-horizontal", "nasa-glover"),
    ("nasa-ochoa-flip", "nasa-ochoa", "flip-horizontal", "nasa-ochoa"),
    ("nasa-glover-rollcw15", "nasa-glover", "rotate-cw-15", "nasa-glover"),
    ("nasa-glover-canvas-top", "nasa-glover", "canvas-top", None),
    ("nasa-glover-canvas-bottom", "nasa-glover", "canvas-bottom", "nasa-glover-canvas-top"),
    ("nasa-almansoori-canvas-top", "nasa-almansoori", "canvas-top", None),
    ("nasa-almansoori-canvas-bottom", "nasa-almansoori", "canvas-bottom", "nasa-almansoori-canvas-top"),
]


def build_derived() -> list[dict]:
    """Transform committed portraits into pose fixtures (no network)."""
    import cv2

    rows = []
    for did, parent, transform, compare_to in DERIVED:
        with Image.open(IMAGES_DIR / f"{parent}.jpg") as im:
            rgb = np.asarray(im.convert("RGB"))
        if transform == "flip-horizontal":
            out = np.ascontiguousarray(rgb[:, ::-1])
        elif transform == "rotate-cw-15":
            h, w = rgb.shape[:2]
            m = cv2.getRotationMatrix2D((w / 2, h / 2), -15.0, 1.0)  # OpenCV angle > 0 is counter-clockwise
            out = cv2.warpAffine(rgb, m, (w, h), flags=cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
        elif transform in ("canvas-top", "canvas-bottom"):
            h, w = rgb.shape[:2]
            out = np.full((2 * h, w, 3), 128, np.uint8)
            y0 = 0 if transform == "canvas-top" else h
            out[y0 : y0 + h] = rgb
        else:
            raise ValueError(transform)
        dest = IMAGES_DIR / f"{did}.jpg"
        Image.fromarray(out).save(dest, "JPEG", quality=JPEG_QUALITY, optimize=False, progressive=False)
        rows.append(
            {
                "id": did,
                "file": f"images/{dest.name}",
                "size": f"{out.shape[1]}x{out.shape[0]}",
                "sha256": hashlib.sha256(dest.read_bytes()).hexdigest(),
                "parent": parent,
                "transform": transform,
                "compareTo": compare_to,
            }
        )
        print(f"ok {did:26s} derived from {parent} ({transform})")
    data = provenance.load()
    data["derived"] = rows
    provenance.save(data)
    return rows
