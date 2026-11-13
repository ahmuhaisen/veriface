"""``facelab models pack --version X``: the versioned, byte-reproducible model pack.

    <out>/veriface-models-<ver>.zip  (+ .sha256)
      veriface-models-<ver>/
        models/manifest.json              byte-identical to the repo's models/manifest.json
        models/<model files>, mp-canonical-face.json, README.txt
        licenses/models/<id>/<LICENSE>    full upstream licence text per model and asset
        NOTICE                            from manifest licence.attribution, plus the web SDK assets
        THIRD-PARTY-LICENSES.txt          one section per licence text, listing the components that use it
        third-party-models.json           machine-readable: id, name, version, spdx, url, sha256, licenceFile
        PACK.json                         name, version, manifestSha256, models [{id, file, sha256}]
        SHA256SUMS                        every file above, `sha256sum -c` format, relative to the pack root

Layout and formats agreed with face-senior-dev (package.ps1 -ModelPack, Docker --model-pack, release-check).
Reproducible: entries sorted, timestamps fixed at 1980-01-01, fixed permissions, deflate level 9, LF line endings.
"""

from __future__ import annotations

import hashlib
import json
import zipfile
from pathlib import Path

from facelab.paths import MANIFEST_PATH, MODELS_DIR, REPO_ROOT
from facelab.upstream import SOURCES, WEB_SDK_ASSETS, sha256_file

DEFAULT_OUT = REPO_ROOT / "service" / "dist"
ZIP_DATE = (1980, 1, 1, 0, 0, 0)
RULE_EQ, RULE_DASH = "=" * 80, "-" * 80

# Which upstream licence file covers which component (by manifest id or web-asset source key).
LICENCE_SOURCE = {"auraface-v1": "auraface_license"}  # everything else: MediaPipe
DEFAULT_LICENCE_SOURCE = "mediapipe_license"


class PackError(RuntimeError):
    pass


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _text(s: str) -> bytes:
    return s.replace("\r\n", "\n").encode("utf-8")


def _licence_bytes(source_key: str) -> tuple[str, bytes]:
    src = SOURCES[source_key]
    if not src.path.exists() or sha256_file(src.path) != src.sha256:
        raise PackError(f"licence file {src.filename} missing or changed: run `python -m facelab models fetch`")
    name = "LICENSE.md" if src.filename.endswith(".md") else "LICENSE"
    return name, src.path.read_bytes().replace(b"\r\n", b"\n")


def build_files(version: str) -> dict[str, bytes]:
    """Every pack file (path relative to the pack root -> bytes), before SHA256SUMS."""
    manifest_bytes = MANIFEST_PATH.read_bytes()
    manifest = json.loads(manifest_bytes)
    files: dict[str, bytes] = {"models/manifest.json": manifest_bytes}
    entries = manifest["models"] + manifest.get("assets", [])
    components = []  # (id, name, version, url, sha256, licence source key, attribution, location)
    for e in entries:
        path = MODELS_DIR / e["file"]
        if not path.exists():
            raise PackError(f"{e['file']} missing: run `python -m facelab models build`")
        data = path.read_bytes()
        if _sha(data) != e["sha256"]:
            raise PackError(f"{e['file']}: SHA-256 does not match manifest.json")
        files[f"models/{e['file']}"] = data
        components.append(
            {
                "id": e["id"],
                "name": e["upstream"]["name"],
                "version": e["upstream"].get("version", ""),
                "url": e["upstream"]["url"],
                "sha256": e["sha256"],
                "spdx": e["licence"]["spdx"],
                "licenceSource": LICENCE_SOURCE.get(e["id"], DEFAULT_LICENCE_SOURCE),
                "attribution": e["licence"].get("attribution", ""),
                "shipped": "model pack",
            }
        )
    for key in WEB_SDK_ASSETS:
        s = SOURCES[key]
        components.append(
            {
                "id": f"web-sdk/{s.filename}",
                "name": s.name,
                "version": s.version,
                "url": s.url,
                "sha256": s.sha256,
                "spdx": "Apache-2.0",
                "licenceSource": DEFAULT_LICENCE_SOURCE,
                "attribution": (
                    f"web-sdk/{s.filename}: {s.name}, {s.version}, from {s.url}. Copyright The MediaPipe Authors "
                    "(Google). Licensed under the Apache License, Version 2.0. Shipped unmodified (served to "
                    "browsers by the web SDK)."
                ),
                "shipped": "web SDK (browser asset; not in this pack)",
            }
        )

    # Licence texts: one per model/asset in the pack, plus grouped full texts for THIRD-PARTY-LICENSES.txt.
    for c in components:
        name, data = _licence_bytes(c["licenceSource"])
        if c["shipped"] == "model pack":
            c["licenceFile"] = f"licenses/models/{c['id']}/{name}"
            files[c["licenceFile"]] = data
        else:
            c["licenceFile"] = None

    files["models/README.txt"] = _text(
        "Model pack for veriface.\n\n"
        "manifest.json lists every model file with its source, licence, review status and SHA-256. The service\n"
        "verifies every SHA-256 at startup and refuses to start on a mismatch. Point paths.models in the\n"
        "configuration at this directory.\n"
    )
    files["NOTICE"] = _text(_notice(version, components))
    files["THIRD-PARTY-LICENSES.txt"] = _text(_third_party(components))
    files["third-party-models.json"] = _text(
        json.dumps(
            [
                {k: c[k] for k in ("id", "name", "version", "spdx", "url", "sha256", "licenceFile", "shipped")}
                for c in components
            ],
            indent=2,
        )
        + "\n"
    )
    files["PACK.json"] = _text(
        json.dumps(
            {
                "name": "veriface-models",
                "version": version,
                "manifestSha256": _sha(manifest_bytes),
                "models": [{"id": e["id"], "file": f"models/{e['file']}", "sha256": e["sha256"]} for e in entries],
            },
            indent=2,
        )
        + "\n"
    )
    return files


def _notice(version: str, components: list[dict]) -> str:
    lines = [
        f"Veriface model pack {version}",
        "",
        "This pack contains machine-learning models and data from third parties, used under the licences listed",
        "below. Full licence texts: licenses/models/<id>/ and THIRD-PARTY-LICENSES.txt.",
        "",
    ]
    for c in components:
        lines += [c["attribution"], ""]
    return "\n".join(lines)


def _third_party(components: list[dict]) -> str:
    out = []
    for c in components:
        _, text = _licence_bytes(c["licenceSource"])
        out += [
            RULE_EQ,
            f"{c['name']}{'' if c['shipped'] == 'model pack' else ' [web SDK browser asset]'} {c['version']}".strip(),
            f"Source:  {c['url']}",
            f"License: {c['spdx']}",
            f"SHA-256: {c['sha256']} (shipped file)",
            RULE_DASH,
            text.decode("utf-8").rstrip("\n"),
            "",
        ]
    return "\n".join(out) + "\n"


def write_pack(version: str, out_dir: Path = DEFAULT_OUT) -> Path:
    files = build_files(version)
    files["SHA256SUMS"] = _text("".join(f"{_sha(data)}  {path}\n" for path, data in sorted(files.items())))
    root = f"veriface-models-{version}"
    out_dir.mkdir(parents=True, exist_ok=True)
    zpath = out_dir / f"{root}.zip"
    tmp = zpath.with_suffix(".zip.part")
    with zipfile.ZipFile(tmp, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for path in sorted(files):
            info = zipfile.ZipInfo(f"{root}/{path}", date_time=ZIP_DATE)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            info.create_system = 3  # unix, so the permission bits are honoured
            z.writestr(info, files[path], compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
    tmp.replace(zpath)
    digest = sha256_file(zpath)
    (out_dir / f"{root}.zip.sha256").write_text(f"{digest}  {zpath.name}\n", encoding="utf-8", newline="\n")
    print(f"wrote {zpath} ({zpath.stat().st_size / 1e6:.1f} MB) sha256 {digest}")
    return zpath
