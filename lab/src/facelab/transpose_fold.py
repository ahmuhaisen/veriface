"""Deterministic, exact Transpose clean-up for tf2onnx output (run without tf2onnx's own optimizer).

tf2onnx wraps every TFLite NHWC op in NHWC<->NCHW Transposes; with its optimizer disabled (it is not
reproducible, see convert.py) hundreds of them remain, and ONNX Runtime does not remove them all (vision agent,
2026-10-01: one NCHW->NCHWc reorder on a [1,32,256,256] tensor cost 34 ms in the segmenter tail).

Two rewrites, repeated until nothing changes; both are exact (no arithmetic on activations):
  1. Transpose of a constant initializer -> a new initializer holding the transposed data.
  2. Transpose(p1) -> Transpose(p2) whose composition is the identity, where the first output has no other
     consumer -> both removed and their consumers rewired to the original tensor.
Iteration follows node order, so the result is byte-reproducible.
"""

from __future__ import annotations

import numpy as np
import onnx
from onnx import numpy_helper


def _perm(node: onnx.NodeProto) -> list[int]:
    return list(next(a for a in node.attribute if a.name == "perm").ints)


def fold_transposes(model: onnx.ModelProto) -> dict[str, int]:
    g = model.graph
    stats = {"constantFolded": 0, "pairsCancelled": 0}
    graph_outputs = {o.name for o in g.output}
    changed = True
    while changed:
        changed = False
        inits = {t.name: t for t in g.initializer}
        consumers: dict[str, list[onnx.NodeProto]] = {}
        for n in g.node:
            for i in n.input:
                consumers.setdefault(i, []).append(n)
        producer = {o: n for n in g.node for o in n.output}

        for n in list(g.node):
            if n.op_type != "Transpose":
                continue
            src = n.input[0]
            # 1. Constant fold.
            if src in inits and n.output[0] not in graph_outputs:
                arr = numpy_helper.to_array(inits[src])
                folded = numpy_helper.from_array(np.ascontiguousarray(np.transpose(arr, _perm(n))), n.output[0])
                g.initializer.append(folded)
                g.node.remove(n)
                if len(consumers.get(src, [])) == 1 and src not in graph_outputs:
                    g.initializer.remove(inits[src])
                stats["constantFolded"] += 1
                changed = True
                break
            # 2. Cancel an inverse pair.
            first = producer.get(src)
            if first is None or first.op_type != "Transpose":
                continue
            if len(consumers.get(first.output[0], [])) != 1 or first.output[0] in graph_outputs:
                continue
            p1, p2 = _perm(first), _perm(n)
            if [p1[i] for i in p2] != list(range(len(p1))):
                continue
            origin, out = first.input[0], n.output[0]
            if out in graph_outputs:
                continue
            for c in consumers.get(out, []):
                c.input[:] = [origin if x == out else x for x in c.input]
            g.node.remove(n)
            g.node.remove(first)
            stats["pairsCancelled"] += 1
            changed = True
            break
    return stats
