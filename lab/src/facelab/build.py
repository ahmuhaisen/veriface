"""``facelab models build``: ONNX files, fidelity (L3) and models/manifest.json."""

from __future__ import annotations

import datetime as dt
import json

import jsonschema
import onnx

from facelab import __version__, assets, convert, fidelity
from facelab.paths import MANIFEST_PATH, MANIFEST_SCHEMA_PATH, MODELS_DIR
from facelab.specs import MODELS, MP_LICENCE_URL, ModelSpec
from facelab.upstream import AURAFACE_REVISION, SOURCES, check_forbidden, fetch_all, sha256_file


class FidelityError(RuntimeError):
    pass


def attribution(spec: ModelSpec) -> str:
    """NOTICE text for one model: origin, copyright, licence, and what the Veriface authors changed (Apache-2.0 section 4(b))."""
    src = SOURCES[spec.source_key]
    if spec.source_key == "auraface":
        return (
            f"{spec.id}: {src.name}, revision {AURAFACE_REVISION}, from {src.url}. Copyright fal.ai. Licensed under "
            "the Apache License, Version 2.0. Modified by the Veriface authors: graph input/output tensors renamed and the batch "
            "dimension made symbolic; weights and operators unchanged."
        )
    member = f" ({spec.member} from the bundle)" if spec.member else ""
    return (
        f"{spec.id}: {src.name}{member}, {src.version}, from {src.url}. Copyright The MediaPipe Authors (Google). "
        f"Licensed under the Apache License, Version 2.0. Modified by the Veriface authors: converted from TFLite to ONNX with "
        f"tf2onnx {convert.tf2onnx_version()}; graph input/output tensors renamed, internal tensor names "
        "canonicalised and the batch dimension fixed to 1; weights and operators unchanged."
    )


def _licence(spec: ModelSpec) -> dict:
    src = SOURCES[spec.source_key]
    return {
        "spdx": "Apache-2.0",
        "url": src.licence_url if spec.source_key == "auraface" else MP_LICENCE_URL,
        "trainingData": {
            "statement": spec.training_statement,
            "commercialUse": spec.commercial_use,
            "source": spec.training_source,
        },
        "attribution": attribution(spec),
    }


def _model_entry(spec: ModelSpec, conversion: dict) -> dict:
    path = MODELS_DIR / spec.file
    src = SOURCES[spec.source_key]
    upstream = {"name": src.name, "version": src.version, "url": src.url, "sha256": src.sha256}
    if spec.member:
        upstream["file"] = spec.member
    review = {"status": "pending"}
    if spec.review_notes:
        review["notes"] = spec.review_notes
    m = onnx.load(str(path), load_external_data=False)
    fp16 = any(t.data_type == onnx.TensorProto.FLOAT16 for t in m.graph.initializer)
    notes = spec.notes
    if fp16:
        notes += (
            " Precision: weights stored as fp16 (as upstream) and Cast to float32 inside the graph; all "
            "computation and every graph input/output is float32."
        )
    return {
        "id": spec.id,
        "role": spec.role,
        **({"variant": spec.variant} if spec.variant else {}),
        "file": spec.file,
        "sha256": sha256_file(path),
        "bytes": path.stat().st_size,
        "format": "onnx",
        "opset": next(o.version for o in m.opset_import if o.domain in ("", "ai.onnx")),
        # Weight storage precision. Activations and all graph I/O are float32 regardless (see io).
        "precision": "fp16" if fp16 else "fp32",
        "upstream": upstream,
        "conversion": conversion,
        "licence": _licence(spec),
        "review": review,
        "io": {"inputs": [t.to_json() for t in spec.inputs], "outputs": [t.to_json() for t in spec.outputs]},
        "notes": notes,
    }


def _canonical_entry() -> dict:
    path = MODELS_DIR / assets.CANONICAL_FILE
    obj, meta = SOURCES["canonical_face_obj"], SOURCES["geometry_pipeline_metadata"]
    return {
        "id": assets.CANONICAL_ID,
        "file": assets.CANONICAL_FILE,
        "sha256": sha256_file(path),
        "bytes": path.stat().st_size,
        "upstream": {"name": obj.name, "version": obj.version, "url": obj.url, "sha256": obj.sha256},
        "licence": {
            "spdx": "Apache-2.0",
            "url": MP_LICENCE_URL,
            "trainingData": {
                "statement": "Geometry asset, not a trained model.",
                "commercialUse": "allowed",
            },
            "attribution": (
                f"{assets.CANONICAL_ID}: {obj.name} and {meta.name}, {obj.version}, from {obj.url} and {meta.url}. "
                "Copyright The MediaPipe Authors (Google). Licensed under the Apache License, Version 2.0. "
                "Modified by the Veriface authors: the 468 vertices and the 33 Procrustes landmark weights extracted into JSON."
            ),
        },
        "notes": (
            "JSON: 'vertices' = the 468 OBJ vertices (cm; vertex i corresponds to landmark i), "
            "'procrustesLandmarkBasis' = the 33 {landmark, weight} pairs MediaPipe uses for its weighted "
            f"Procrustes fit, parsed from {meta.url} (sha256 {meta.sha256}). Pose angles and sign conventions: "
            "testdata/golden/README.md and contracts/checks.yaml."
        ),
    }


def build_all(*, skip_fidelity: bool = False) -> dict:
    fetch_all()
    tool_version = f"tf2onnx {convert.tf2onnx_version()}"
    entries = []
    failures = []
    landmarker_onnx = None
    for spec in MODELS:
        dest = MODELS_DIR / spec.file
        command = convert.build_onnx(spec, dest)
        if spec.role == "face-landmarker":
            landmarker_onnx = dest
        conversion = {
            "tool": "tf2onnx" if spec.converted else "onnx (I/O rename only)",
            "toolVersion": tool_version if spec.converted else f"onnx {onnx.__version__}",
            "command": command,
        }
        if skip_fidelity:
            conversion["fidelity"] = "NOT RUN (--skip-fidelity)"
        else:
            res = fidelity.check(spec, convert.upstream_file(spec), dest, landmarker_onnx)
            conversion["fidelity"] = res.summary()
            print(f"{'PASS' if res.ok else 'FAIL'} {spec.id}: {res.summary()}")
            if not res.ok:
                failures.append(spec.id)
        entries.append(_model_entry(spec, conversion))

    assets.build_canonical_face(MODELS_DIR / assets.CANONICAL_FILE)
    manifest = {
        "manifestVersion": 1,
        "generatedAt": dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
        "generator": f"facelab {__version__}",
        "models": entries,
        "assets": [_canonical_entry()],
    }
    schema = json.loads(MANIFEST_SCHEMA_PATH.read_text(encoding="utf-8"))
    jsonschema.Draft202012Validator(schema, format_checker=jsonschema.FormatChecker()).validate(manifest)
    if failures:
        raise FidelityError(f"fidelity above tolerance for {failures}; manifest NOT written")
    MANIFEST_PATH.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    check_forbidden()
    print(f"wrote {MANIFEST_PATH} ({len(entries)} models, 1 asset)")
    return manifest
