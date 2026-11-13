"""Pinned upstream artifacts and the fetcher.

Every source has a fixed URL (commit- or revision-pinned where the host allows it) and the SHA-256 we
recorded on first fetch. A mismatch is a hard failure: upstream must never change silently under us.

Licence policy (AGENTS.md section 2): from the AuraFace repository we fetch the recognition model ONLY.
Its other files (SCRFD, 2d106det, 1k3d68, genderage) are InsightFace's non-commercial models and must
never be downloaded or kept. ``FORBIDDEN_NAMES`` enforces that on the download cache.
"""

from __future__ import annotations

import hashlib
import shutil
import urllib.request
import zipfile
from dataclasses import dataclass
from pathlib import Path

from facelab.paths import UPSTREAM_DIR

MP_LEGACY_COMMIT = "45005ba4e32050ad1d387d3d783c853bb56d3cb7"  # google-ai-edge/mediapipe master, 2026-10-01
MP_LEGACY_RAW = f"https://raw.githubusercontent.com/google-ai-edge/mediapipe/{MP_LEGACY_COMMIT}"
MP_MODELS = "https://storage.googleapis.com/mediapipe-models"
MP_ASSETS = "https://storage.googleapis.com/mediapipe-assets"
AURAFACE_REVISION = "af6d057c9b0ec4071d4c49c80e3539258798b609"  # fal/AuraFace-v1, 2024-08-26

# Files that must never appear in the cache (InsightFace non-commercial models).
FORBIDDEN_NAMES = frozenset(
    {
        "scrfd_10g_bnkps.onnx",
        "2d106det.onnx",
        "1k3d68.onnx",
        "genderage.onnx",
        "det_10g.onnx",
        "w600k_r50.onnx",
    }
)


@dataclass(frozen=True)
class Source:
    key: str
    filename: str
    url: str
    sha256: str
    name: str
    version: str
    licence_url: str

    @property
    def path(self) -> Path:
        return UPSTREAM_DIR / self.filename


SOURCES: dict[str, Source] = {
    s.key: s
    for s in [
        Source(
            key="face_landmarker_task",
            filename="face_landmarker.task",
            url=f"{MP_MODELS}/face_landmarker/face_landmarker/float16/1/face_landmarker.task",
            sha256="64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff",
            name="MediaPipe Face Landmarker (task bundle)",
            version="float16/1",
            licence_url="https://ai.google.dev/edge/mediapipe/solutions/vision/face_landmarker#models",
        ),
        Source(
            key="blaze_face_short_range",
            filename="blaze_face_short_range.tflite",
            url=f"{MP_MODELS}/face_detector/blaze_face_short_range/float16/1/blaze_face_short_range.tflite",
            sha256="b4578f35940bf5a1a655214a1cce5cab13eba73c1297cd78e1a04c2380b0152f",
            name="MediaPipe BlazeFace short-range",
            version="float16/1",
            licence_url="https://ai.google.dev/edge/mediapipe/solutions/vision/face_detector#models",
        ),
        Source(
            key="face_detection_full_range",
            filename="face_detection_full_range.tflite",
            # URL and SHA-256 as pinned by upstream itself in third_party/external_files.bzl at MP_LEGACY_COMMIT.
            url=f"{MP_ASSETS}/modules/face_detection/face_detection_full_range.tflite?generation=1782183493595806",
            sha256="99bf9494d84f50acc6617d89873f71bf6635a841ea699c17cb3377f9507cfec3",
            name="MediaPipe BlazeFace full-range (legacy face_detection module)",
            version=f"mediapipe-assets generation 1782183493595806 (external_files.bzl @ {MP_LEGACY_COMMIT[:12]})",
            licence_url="https://github.com/google-ai-edge/mediapipe/blob/master/LICENSE",
        ),
        Source(
            # Same network packaged with TFLite metadata, so MediaPipe Tasks FaceDetector can run it (goldens).
            key="face_detection_full_range_with_metadata",
            filename="face_detection_full_range_with_metadata.tflite",
            url=f"{MP_ASSETS}/tasks/testdata/vision/face_detection_full_range_with_metadata.tflite"
            "?generation=1782229502308028",
            sha256="f5fd43a368d0eab9873f021eb741223a3e015759a46240a8dddf27af2def3f3e",
            name="MediaPipe BlazeFace full-range with metadata (Tasks test asset; golden runs only)",
            version=f"mediapipe-assets generation 1782229502308028 (external_files.bzl @ {MP_LEGACY_COMMIT[:12]})",
            licence_url="https://github.com/google-ai-edge/mediapipe/blob/master/LICENSE",
        ),
        Source(
            key="selfie_multiclass",
            filename="selfie_multiclass_256x256.tflite",
            url=f"{MP_MODELS}/image_segmenter/selfie_multiclass_256x256/float32/1/selfie_multiclass_256x256.tflite",
            sha256="c6748b1253a99067ef71f7e26ca71096cd449baefa8f101900ea23016507e0e0",
            name="MediaPipe selfie_multiclass_256x256",
            version="float32/1",
            licence_url="https://ai.google.dev/edge/mediapipe/solutions/vision/image_segmenter#multiclass-model",
        ),
        Source(
            key="canonical_face_obj",
            filename="canonical_face_model.obj",
            url=f"{MP_LEGACY_RAW}/mediapipe/modules/face_geometry/data/canonical_face_model.obj",
            sha256="8bac80443397e113f41a8b565ea72c59390bc031d9defab289dba7bc0c54e618",
            name="MediaPipe canonical face model",
            version=f"git {MP_LEGACY_COMMIT[:12]}",
            licence_url="https://github.com/google-ai-edge/mediapipe/blob/master/LICENSE",
        ),
        Source(
            key="geometry_pipeline_metadata",
            filename="geometry_pipeline_metadata_landmarks.pbtxt",
            url=f"{MP_LEGACY_RAW}/mediapipe/modules/face_geometry/data/geometry_pipeline_metadata_landmarks.pbtxt",
            sha256="55ca7c4c7bf195673b9d12a1b5d49d4b9c82b5ade8fed4b565496590866e359d",
            name="MediaPipe face geometry pipeline metadata (Procrustes landmark weights)",
            version=f"git {MP_LEGACY_COMMIT[:12]}",
            licence_url="https://github.com/google-ai-edge/mediapipe/blob/master/LICENSE",
        ),
        Source(
            key="auraface",
            filename="auraface_v1_glintr100.onnx",
            url=f"https://huggingface.co/fal/AuraFace-v1/resolve/{AURAFACE_REVISION}/glintr100.onnx",
            sha256="a7933ea5330113b01c9b60351d8f4c33003f145d8470ac5f0e52ee2effe25c60",  # HF LFS oid
            name="fal/AuraFace-v1 recognition model (glintr100.onnx)",
            version=f"hf {AURAFACE_REVISION[:12]}",
            licence_url="https://huggingface.co/fal/AuraFace-v1/blob/main/LICENSE.md",
        ),
        # Full upstream licence texts, shipped in the model pack (licenses/models/<id>/).
        Source(
            key="mediapipe_license",
            filename="mediapipe-LICENSE",
            url=f"{MP_LEGACY_RAW}/LICENSE",
            sha256="8707eef0533987efc5b155d64761eeb6e20793f50b9bd1a68dad1cf4719d0ed8",
            name="MediaPipe LICENSE (Apache-2.0 plus appended third-party notices)",
            version=f"git {MP_LEGACY_COMMIT[:12]}",
            licence_url="https://github.com/google-ai-edge/mediapipe/blob/master/LICENSE",
        ),
        Source(
            key="auraface_license",
            filename="auraface-LICENSE.md",
            url=f"https://huggingface.co/fal/AuraFace-v1/raw/{AURAFACE_REVISION}/LICENSE.md",
            sha256="609e2cb599f84aaa41d8ef29d8fdb04d164fab22e8d9292ca34a599d0f56a338",
            name="fal/AuraFace-v1 LICENSE.md (Apache-2.0)",
            version=f"hf {AURAFACE_REVISION[:12]}",
            licence_url="https://huggingface.co/fal/AuraFace-v1/blob/main/LICENSE.md",
        ),
    ]
}

# Upstream files the web SDK also ships to browsers (sdk-web/scripts/fetch-models.mjs pins the same URL + hash;
# tests/test_models.py checks they agree).
WEB_SDK_ASSETS = ("face_landmarker_task", "blaze_face_short_range")


def upstream_file(spec) -> Path:  # spec: facelab.specs.ModelSpec
    """The single model file for ``spec``, extracting it from a .task bundle if needed."""
    src = SOURCES[spec.source_key].path
    if spec.member is None:
        return src
    out_dir = UPSTREAM_DIR / src.stem
    out = out_dir / spec.member
    if not out.exists():
        out_dir.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(src) as z:
            z.extract(spec.member, out_dir)
    return out


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


class IntegrityError(RuntimeError):
    pass


def check_forbidden(directory: Path = UPSTREAM_DIR) -> None:
    if not directory.exists():
        return
    bad = [p.name for p in directory.rglob("*") if p.name in FORBIDDEN_NAMES]
    if bad:
        raise IntegrityError(f"Forbidden InsightFace files present in {directory}: {bad}. Delete them.")


def fetch(source: Source, *, force: bool = False, record: bool = False) -> Path:
    """Download ``source`` into the cache and verify its SHA-256.

    ``record`` accepts an unpinned (``PENDING``) source and prints the hash so it can be pinned here.
    """
    UPSTREAM_DIR.mkdir(parents=True, exist_ok=True)
    dest = source.path
    if force or not dest.exists():
        tmp = dest.with_suffix(dest.suffix + ".part")
        req = urllib.request.Request(source.url, headers={"User-Agent": "facelab"})
        with urllib.request.urlopen(req, timeout=120) as resp, tmp.open("wb") as out:
            shutil.copyfileobj(resp, out)
        tmp.replace(dest)
    actual = sha256_file(dest)
    if source.sha256 == "PENDING":
        if not record:
            raise IntegrityError(f"{source.key}: no pinned SHA-256 (got {actual}). Pin it in upstream.py.")
        print(f"RECORD {source.key} sha256={actual}")
    elif actual != source.sha256:
        raise IntegrityError(
            f"{source.key}: SHA-256 mismatch for {source.url}\n  expected {source.sha256}\n  actual   {actual}"
        )
    return dest


def fetch_all(*, force: bool = False, record: bool = False) -> dict[str, Path]:
    check_forbidden()
    paths = {key: fetch(src, force=force, record=record) for key, src in SOURCES.items()}
    check_forbidden()
    return paths
