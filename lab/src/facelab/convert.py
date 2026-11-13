"""TFLite -> ONNX conversion and the small graph edits we allow on shipped models.

The only edits are: renaming graph I/O tensors to the names in ``specs`` and pinning the batch dimension
(1 for the MediaPipe models, symbolic ``N`` for AuraFace). Weights and ops are never touched here.
"""

from __future__ import annotations

from pathlib import Path
from unittest import mock

import onnx

from facelab.specs import ModelSpec
from facelab.upstream import upstream_file

OPSET = 17


def tf2onnx_version() -> str:
    import tf2onnx

    return tf2onnx.__version__


def _rename(model: onnx.ModelProto, mapping: dict[str, str]) -> None:
    if not mapping:
        return
    g = model.graph
    for node in g.node:
        node.input[:] = [mapping.get(n, n) for n in node.input]
        node.output[:] = [mapping.get(n, n) for n in node.output]
    for coll in (g.input, g.output, g.value_info):
        for vi in coll:
            vi.name = mapping.get(vi.name, vi.name)


def _set_batch(model: onnx.ModelProto, batch: int | str) -> None:
    for vi in list(model.graph.input) + list(model.graph.output):
        dims = vi.type.tensor_type.shape.dim
        if not dims:
            continue
        d0 = dims[0]
        if isinstance(batch, int):
            d0.ClearField("dim_param")
            d0.dim_value = batch
        else:
            d0.ClearField("dim_value")
            d0.dim_param = batch


def _canonicalize(model: onnx.ModelProto) -> None:
    """Make tf2onnx output byte-reproducible.

    tf2onnx names nodes and folded constants from a process-global counter and emits initializers in an
    unstable order, so two conversions of the same file hash differently. Rename every internal node and
    tensor by its position in the (stable) node order, and order initializers by first use. Graph I/O
    names are kept.
    """
    g = model.graph
    keep = {vi.name for vi in g.input} | {vi.name for vi in g.output}
    mapping: dict[str, str] = {}
    inits = {i.name for i in g.initializer}

    def canon(name: str) -> str:
        if not name or name in keep:
            return name
        if name not in mapping:
            mapping[name] = f"{'w' if name in inits else 't'}{len(mapping)}"
        return mapping[name]

    for i, node in enumerate(g.node):
        node.name = f"n{i}_{node.op_type}"
        node.input[:] = [canon(n) for n in node.input]
        node.output[:] = [canon(n) for n in node.output]
    ordered = sorted(g.initializer, key=lambda t: int(mapping[t.name][1:]) if t.name in mapping else 1 << 30)
    for t in ordered:
        t.name = mapping.get(t.name, t.name)
    new = [onnx.TensorProto() for _ in ordered]
    for dst, src in zip(new, ordered, strict=True):
        dst.CopyFrom(src)
    del g.initializer[:]
    g.initializer.extend(new)


def _strip_value_info(model: onnx.ModelProto) -> None:
    # tf2onnx leaves intermediate shapes referencing old symbolic dims; ORT re-infers them.
    del model.graph.value_info[:]


def build_onnx(spec: ModelSpec, dest: Path) -> str:
    """Write ``dest`` for ``spec``; return the conversion command string for the manifest."""
    src = upstream_file(spec)
    if spec.converted:
        import tf2onnx.convert
        from tf2onnx import optimizer as tf2onnx_optimizer

        # optimizers={}: tf2onnx's graph optimizer iterates identity-hashed sets, so its output varies run to run
        # (node count and constant folding differ). Without it the conversion is byte-reproducible; fp16 weights
        # stay fp16 behind Cast nodes, which ONNX Runtime folds at session load (same latency, half the size).
        # from_tflite() does not expose `optimizers`, so pass it to the optimizer call it makes.
        real = tf2onnx_optimizer.optimize_graph
        with mock.patch.object(
            tf2onnx_optimizer,
            "optimize_graph",
            lambda g, catch_errors=True, optimizers=None: real(g, catch_errors, optimizers={}),
        ):
            model, _ = tf2onnx.convert.from_tflite(str(src), opset=OPSET)
        command = (
            f"tf2onnx.convert.from_tflite('{src.name}', opset={OPSET}) with optimize_graph(optimizers={{}}); "
            "rename I/O; canonical internal names; batch=1"
        )
        _strip_value_info(model)
        _rename(model, spec.rename)
        _canonicalize(model)
        _set_batch(model, 1)
        model.producer_name = "facelab (tf2onnx)"
    else:
        model = onnx.load(str(src))
        command = "upstream ONNX; rename I/O; symbolic batch N"
        _rename(model, spec.rename)
        _set_batch(model, "N")
    model.doc_string = f"Veriface model {spec.id}; see models/manifest.json."
    onnx.checker.check_model(model)
    names_in = [i.name for i in model.graph.input if i.name not in {t.name for t in model.graph.initializer}]
    names_out = [o.name for o in model.graph.output]
    want_in = [t.name for t in spec.inputs]
    want_out = [t.name for t in spec.outputs]
    if names_in != want_in or names_out != want_out:
        raise RuntimeError(f"{spec.id}: I/O {names_in}->{names_out}, spec says {want_in}->{want_out}")
    dest.parent.mkdir(parents=True, exist_ok=True)
    onnx.save(model, str(dest))
    return command
