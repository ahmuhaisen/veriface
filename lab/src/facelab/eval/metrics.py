"""Verification metrics (ISO/IEC 19795-1 vocabulary) and differential-performance summaries (19795-10 style).

Scores: higher = more similar. A comparison "matches" when score >= threshold.
All functions take optional per-comparison weights so subject-level bootstrap can reuse them.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np
from scipy.stats import beta

CONFIDENCE = 0.95


def _w(x: np.ndarray, w: np.ndarray | None) -> np.ndarray:
    return np.ones_like(x, dtype=np.float64) if w is None else np.asarray(w, np.float64)


def fmr(impostor: np.ndarray, t: float, w: np.ndarray | None = None) -> float:
    w = _w(impostor, w)
    return float(w[impostor >= t].sum() / w.sum()) if w.sum() else float("nan")


def fnmr(genuine: np.ndarray, t: float, w: np.ndarray | None = None) -> float:
    w = _w(genuine, w)
    return float(w[genuine < t].sum() / w.sum()) if w.sum() else float("nan")


def threshold_for_fmr(impostor: np.ndarray, target: float, w: np.ndarray | None = None) -> float:
    """Lowest threshold whose FMR is <= target (ties at the threshold count as matches)."""
    order = np.argsort(-impostor, kind="stable")
    s, ww = impostor[order], _w(impostor, w)[order]
    allowed = target * ww.sum()
    cum = np.cumsum(ww)
    # Largest prefix of the highest impostor scores we may let through.
    k = int(np.searchsorted(cum, allowed + 1e-12, side="right"))  # scores s[:k] may match
    if k >= len(s):
        return float(s[-1])
    t = float(np.nextafter(s[k], np.inf))
    while k > 0 and fmr(impostor, t, w) > target + 1e-12:  # ties straddling the cut
        k -= 1
        t = float(np.nextafter(s[k], np.inf))
    return t


def clopper_pearson_upper(errors: int, n: int, confidence: float = CONFIDENCE) -> float:
    """One-sided exact upper bound on an error rate. errors = 0 gives 1 - (1-c)^(1/n), about 3/n ("rule of 3")."""
    if n == 0:
        return 1.0
    if errors >= n:
        return 1.0
    return float(beta.ppf(confidence, errors + 1, n - errors))


def comparisons_needed(target_fmr: float, confidence: float = CONFIDENCE) -> int:
    """Impostor comparisons needed to support FMR <= target with zero false matches (rule of 3 at 95%)."""
    return math.ceil(-math.log(1 - confidence) / target_fmr)


def det_curve(genuine: np.ndarray, impostor: np.ndarray) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    ts = np.unique(np.concatenate([genuine, impostor, [np.inf]]))
    f = np.array([fmr(impostor, t) for t in ts])
    n = np.array([fnmr(genuine, t) for t in ts])
    return ts, f, n


def eer(genuine: np.ndarray, impostor: np.ndarray) -> tuple[float, float]:
    """(EER, threshold), linearly interpolated where FMR - FNMR changes sign."""
    ts, f, n = det_curve(genuine, impostor)
    d = f - n
    i = int(np.argmax(d <= 0))  # first threshold where FNMR >= FMR (f decreases, n increases with t)
    if i == 0:
        return float((f[0] + n[0]) / 2), float(ts[0])
    a = d[i - 1] / (d[i - 1] - d[i]) if d[i - 1] != d[i] else 0.5
    e = f[i - 1] + a * (f[i] - f[i - 1])
    t = ts[i - 1] + a * (ts[i] - ts[i - 1]) if np.isfinite(ts[i]) else ts[i - 1]
    return float(e), float(t)


def claimable_threshold(impostor: np.ndarray, target: float, confidence: float = CONFIDENCE) -> float | None:
    """Lowest threshold at which the one-sided exact upper bound on FMR is <= target, or None if the sample is
    too small to support the target at all (fewer than comparisons_needed(target) impostor comparisons)."""
    n = len(impostor)
    s = np.sort(impostor)[::-1]
    k = int(target * n)  # false matches we could possibly afford
    while k >= 0 and clopper_pearson_upper(k, n, confidence) > target:
        k -= 1
    if k < 0:
        return None
    t = float(np.nextafter(s[k], np.inf)) if k < n else float(s[-1])
    while (impostor >= t).sum() > k:  # ties
        k -= 1
        if k < 0:
            return None
        t = float(np.nextafter(s[k], np.inf))
    return t


@dataclass
class OperatingPoint:
    target_fmr: float
    impostor_n: int
    genuine_n: int
    empirical: dict  # threshold where measured FMR <= target
    claimable: dict | None  # threshold where the 95% upper bound on FMR <= target (None: sample too small)
    needed: int  # impostor comparisons needed for any claim at this target (rule of 3)
    threshold_ci: tuple[float, float] | None = None  # subject-bootstrap CI of the empirical threshold

    def to_json(self) -> dict:
        return {
            "targetFmr": self.target_fmr,
            "impostorComparisons": self.impostor_n,
            "genuineComparisons": self.genuine_n,
            "comparisonsNeededForClaim": self.needed,
            "empirical": self.empirical,
            "claimable": self.claimable,
            "empiricalThresholdCi95SubjectBootstrap": None
            if self.threshold_ci is None
            else [round(v, 4) for v in self.threshold_ci],
        }


def _at(genuine: np.ndarray, impostor: np.ndarray, t: float) -> dict:
    fm = int((impostor >= t).sum())
    fnm = int((genuine < t).sum())
    return {
        "threshold": round(t, 4),
        "falseMatches": fm,
        "fmr": fmr(impostor, t),
        "fmrUpper95": clopper_pearson_upper(fm, len(impostor)),
        "falseNonMatches": fnm,
        "fnmr": fnmr(genuine, t),
        "fnmrUpper95": clopper_pearson_upper(fnm, len(genuine)),
    }


def operating_point(genuine: np.ndarray, impostor: np.ndarray, target: float) -> OperatingPoint:
    t_emp = threshold_for_fmr(impostor, target)
    t_claim = claimable_threshold(impostor, target)
    return OperatingPoint(
        target,
        len(impostor),
        len(genuine),
        _at(genuine, impostor, t_emp),
        None if t_claim is None else _at(genuine, impostor, t_claim),
        comparisons_needed(target),
    )


def subject_bootstrap_threshold(
    gen_subj: np.ndarray,
    imp_subj: np.ndarray,
    impostor: np.ndarray,
    target: float,
    n_boot: int = 500,
    seed: int = 0,
) -> tuple[float, float]:
    """95% percentile CI of the threshold, resampling SUBJECTS (comparisons sharing a subject are dependent).

    imp_subj: (N, 2) subject indices of each impostor comparison. A comparison's weight is the product of how
    many times each of its subjects was drawn.
    """
    rng = np.random.default_rng(seed)
    subjects = np.unique(np.concatenate([gen_subj.ravel(), imp_subj.ravel()]))
    index = {s: i for i, s in enumerate(subjects)}
    a = np.array([index[s] for s in imp_subj[:, 0]])
    b = np.array([index[s] for s in imp_subj[:, 1]])
    ts = []
    for _ in range(n_boot):
        counts = np.bincount(rng.integers(0, len(subjects), len(subjects)), minlength=len(subjects))
        w = counts[a] * counts[b]
        if w.sum() == 0:
            continue
        ts.append(threshold_for_fmr(impostor, target, w))
    lo, hi = np.percentile(ts, [2.5, 97.5])
    return float(lo), float(hi)


def group_differentials(
    genuine: np.ndarray,
    gen_groups: np.ndarray,
    impostor: np.ndarray,
    imp_groups: np.ndarray,
    t: float,
) -> dict:
    """Per-group FMR (within-group impostors) and FNMR at a fixed threshold, with exact 95% upper bounds.

    imp_groups: group of each impostor comparison when both subjects share it, else None (excluded).
    """
    out: dict = {"groups": {}}
    groups = sorted({g for g in gen_groups if g is not None} | {g for g in imp_groups if g is not None})
    for g in groups:
        gi = impostor[imp_groups == g]
        gg = genuine[gen_groups == g]
        fm, fnm = int((gi >= t).sum()), int((gg < t).sum())
        out["groups"][g] = {
            "impostorComparisons": len(gi),
            "falseMatches": fm,
            "fmr": fm / len(gi) if len(gi) else None,
            "fmrUpper95": clopper_pearson_upper(fm, len(gi)) if len(gi) else None,
            "genuineComparisons": len(gg),
            "falseNonMatches": fnm,
            "fnmr": fnm / len(gg) if len(gg) else None,
            "fnmrUpper95": clopper_pearson_upper(fnm, len(gg)) if len(gg) else None,
        }

    def ratio(key: str) -> float | None:
        vals = [v[key] for v in out["groups"].values() if v[key] is not None]
        if len(vals) < 2 or max(vals) == 0:
            return None
        return float("inf") if min(vals) == 0 else max(vals) / min(vals)

    out["fmrMaxMinRatio"] = ratio("fmr")
    out["fnmrMaxMinRatio"] = ratio("fnmr")
    return out


def cohen_kappa(a: list[str], b: list[str]) -> float | None:
    """Cohen's kappa for two annotators' pass/fail labels on the same samples (None if undefined)."""
    if not a:
        return None
    a_arr, b_arr = np.asarray(a), np.asarray(b)
    po = float(np.mean(a_arr == b_arr))
    pe = sum(float(np.mean(a_arr == c)) * float(np.mean(b_arr == c)) for c in ("pass", "fail"))
    return None if pe == 1.0 else (po - pe) / (1.0 - pe)


def confusion(labels: list[str], predicted: list[str]) -> dict:
    """Quality check: ground-truth label (pass/fail) x predicted status (pass/fail/undetermined/skipped)."""
    cats = ["pass", "fail", "undetermined", "skipped"]
    m = {lab: dict.fromkeys(cats, 0) for lab in ("pass", "fail")}
    for lab, p in zip(labels, predicted, strict=True):
        m[lab][p] += 1
    n_pass, n_fail = sum(m["pass"].values()), sum(m["fail"].values())
    return {
        "matrix": m,
        "falseRejectRate": m["pass"]["fail"] / n_pass if n_pass else None,  # compliant sample flagged as failing
        "falseAcceptRate": m["fail"]["pass"] / n_fail if n_fail else None,  # non-compliant sample let through
        "undeterminedRate": (m["pass"]["undetermined"] + m["fail"]["undetermined"]) / max(n_pass + n_fail, 1),
    }
