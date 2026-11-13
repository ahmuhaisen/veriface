"""L7 metrics on synthetic score arrays (no images, no real data)."""

from __future__ import annotations

import numpy as np
import pytest

from facelab.eval import metrics as m


def test_fmr_fnmr_at_threshold():
    imp = np.array([0.1, 0.2, 0.3, 0.4])
    gen = np.array([0.35, 0.5, 0.9])
    assert m.fmr(imp, 0.3) == 0.5  # 0.3 and 0.4 match (>=)
    assert m.fnmr(gen, 0.4) == pytest.approx(1 / 3)


def test_threshold_for_fmr_respects_target_and_ties():
    imp = np.array([0.9, 0.5, 0.5, 0.1] + [0.0] * 96)  # 100 impostors
    t = m.threshold_for_fmr(imp, 0.01)  # 1 false match allowed
    assert m.fmr(imp, t) <= 0.01 and t > 0.5
    t = m.threshold_for_fmr(imp, 0.02)  # 2 allowed, but the next two are tied at 0.5: cannot admit just one
    assert m.fmr(imp, t) == 0.01
    t = m.threshold_for_fmr(imp, 0.03)
    assert m.fmr(imp, t) == 0.03


def test_rule_of_three():
    assert m.clopper_pearson_upper(0, 3000) == pytest.approx(3 / 3000, rel=0.01)
    assert m.comparisons_needed(1e-4) == 29958  # -ln(0.05)/1e-4, i.e. ~3/FMR
    assert m.clopper_pearson_upper(5, 5) == 1.0


def test_claimable_threshold_needs_enough_comparisons():
    rng = np.random.default_rng(0)
    small = rng.normal(0, 0.1, 1000)
    assert m.claimable_threshold(small, 1e-3) is None  # 1000 < 2996 comparisons needed
    big = rng.normal(0, 0.1, 50_000)
    t = m.claimable_threshold(big, 1e-3)
    fm = int((big >= t).sum())
    assert m.clopper_pearson_upper(fm, len(big)) <= 1e-3
    assert t >= m.threshold_for_fmr(big, 1e-3)  # a claim costs threshold


def test_eer_symmetric_gaussians():
    rng = np.random.default_rng(1)
    gen, imp = rng.normal(1, 1, 20_000), rng.normal(-1, 1, 20_000)
    e, t = m.eer(gen, imp)
    assert e == pytest.approx(0.1587, abs=0.01)  # Phi(-1)
    assert t == pytest.approx(0.0, abs=0.05)


def test_subject_bootstrap_ci_contains_estimate():
    rng = np.random.default_rng(2)
    subj = np.arange(60)
    pairs = np.array([(a, b) for a in subj for b in subj if a < b])
    imp = rng.normal(0, 0.1, len(pairs))
    lo, hi = m.subject_bootstrap_threshold(subj, pairs, imp, 0.01, n_boot=200)
    t = m.threshold_for_fmr(imp, 0.01)
    assert lo <= t <= hi and hi > lo


def test_group_differentials_ratio():
    imp = np.array([0.9, 0.1, 0.1, 0.1, 0.9, 0.9, 0.1, 0.1])
    groups = np.array(["a", "a", "a", "a", "b", "b", "b", "b"], dtype=object)
    gen = np.array([0.9, 0.9, 0.2, 0.9])
    ggroups = np.array(["a", "a", "b", "b"], dtype=object)
    d = m.group_differentials(gen, ggroups, imp, groups, 0.5)
    assert d["groups"]["a"]["fmr"] == 0.25 and d["groups"]["b"]["fmr"] == 0.5
    assert d["fmrMaxMinRatio"] == 2.0
    assert d["fnmrMaxMinRatio"] == float("inf")  # group a has no false non-matches


def test_confusion_matrix():
    c = m.confusion(["pass", "pass", "fail", "fail"], ["pass", "fail", "pass", "undetermined"])
    assert c["falseRejectRate"] == 0.5 and c["falseAcceptRate"] == 0.5
    assert c["undeterminedRate"] == 0.25
