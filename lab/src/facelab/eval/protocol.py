"""Comparison protocol: probe x reference pairs, genuine by subject identity.

Impostor sets:
  - ``all``: every cross-subject probe x reference pair (zero-effort impostors, ISO/IEC 19795-1).
  - ``matched``: only pairs whose subjects agree on every ``match_on`` attribute (e.g. sex, ageBand, skinTone).
    This is the demographically matched impostor set: the realistic attack and the harder case, because
    demographically similar people score higher (NIST FRVT Part 3; DHS RIVTD). Thresholds must be chosen on it.
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

from facelab.eval.dataset import Dataset


@dataclass
class Comparisons:
    probe: list[str]
    reference: list[str]
    probe_subject: np.ndarray
    reference_subject: np.ndarray
    genuine: np.ndarray  # bool

    def __len__(self) -> int:
        return len(self.probe)

    def subset(self, keep: np.ndarray) -> Comparisons:
        idx = np.flatnonzero(keep)
        return Comparisons(
            [self.probe[i] for i in idx],
            [self.reference[i] for i in idx],
            self.probe_subject[idx],
            self.reference_subject[idx],
            self.genuine[idx],
        )


def build(ds: Dataset, usable: set[str], impostors: str = "all", match_on: tuple[str, ...] = ()) -> Comparisons:
    probes = [s for s in ds.samples if s.role == "probe" and s.id in usable]
    refs = [s for s in ds.samples if s.role == "reference" and s.id in usable]
    rows = []
    for p in probes:
        for r in refs:
            same = p.subject == r.subject
            if not same:
                if impostors == "matched":
                    ap, ar = ds.subjects[p.subject], ds.subjects[r.subject]
                    if any(k not in ap or k not in ar or ap[k] != ar[k] for k in match_on):
                        continue
                elif impostors != "all":
                    raise ValueError(impostors)
            rows.append((p.id, r.id, p.subject, r.subject, same))
    if not rows:
        return Comparisons([], [], np.array([]), np.array([]), np.array([], bool))
    pi, ri, ps, rs, g = zip(*rows, strict=True)
    return Comparisons(list(pi), list(ri), np.array(ps, object), np.array(rs, object), np.array(g, bool))


def group_of(ds: Dataset, subject: str, keys: tuple[str, ...]) -> str | None:
    attrs = ds.subjects[subject]
    if any(k not in attrs for k in keys):
        return None
    return "/".join(f"{k}={attrs[k]}" for k in keys)
