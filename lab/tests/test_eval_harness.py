"""L7 harness: data-safety guards, protocol and pose-bias estimation. Synthetic inputs only."""

from __future__ import annotations

import numpy as np
import pytest
import yaml

from facelab.eval import dataset, pitch, protocol
from facelab.paths import REPO_ROOT, TESTDATA_DIR, WORK_DIR


def _manifest(tmp_path, root, **extra):
    m = {
        "formatVersion": 1,
        "root": str(root),
        "subjects": [
            {"id": "a", "attributes": {"sex": "F", "ageBand": "30-39"}},
            {"id": "b", "attributes": {"sex": "F", "ageBand": "30-39"}},
            {"id": "c", "attributes": {"sex": "M", "ageBand": "30-39"}},
        ],
        "samples": [
            {"id": f"{s}-{r}", "subject": s, "path": f"{s}-{r}.jpg", "role": r}
            for s in "abc"
            for r in ("probe", "reference")
        ],
        **extra,
    }
    p = tmp_path / "m.yaml"
    p.write_text(yaml.safe_dump(m))
    return p


def test_real_data_root_inside_repo_is_refused(tmp_path):
    with pytest.raises(dataset.DatasetError, match="inside the repository"):
        dataset.load(_manifest(tmp_path, REPO_ROOT / "data", consent="x"))


def test_fixtures_root_allowed_only_when_illustrative(tmp_path):
    with pytest.raises(dataset.DatasetError):
        dataset.load(_manifest(tmp_path, TESTDATA_DIR, consent="x"))
    assert dataset.load(_manifest(tmp_path, TESTDATA_DIR, illustrative=True)).illustrative


def test_consent_required_for_real_data(tmp_path):
    with pytest.raises(dataset.DatasetError, match="consent"):
        dataset.load(_manifest(tmp_path, tmp_path / "data"))


def test_output_dir_guard():
    with pytest.raises(dataset.DatasetError):
        dataset.check_output_dir(REPO_ROOT / "testdata" / "eval-out")
    dataset.check_output_dir(WORK_DIR / "eval-x")  # git-ignored scratch is fine


def test_protocol_genuine_and_matched_impostors(tmp_path):
    ds = dataset.load(_manifest(tmp_path, tmp_path / "data", consent="DPA-1"))
    usable = {s.id for s in ds.samples}
    all_ = protocol.build(ds, usable, "all")
    assert len(all_) == 9 and all_.genuine.sum() == 3
    matched = protocol.build(ds, usable, "matched", ("sex", "ageBand"))
    imp = ~matched.genuine
    assert matched.genuine.sum() == 3 and imp.sum() == 2  # only a<->b are demographically matched
    assert set(zip(matched.probe_subject[imp], matched.reference_subject[imp], strict=True)) == {("a", "b"), ("b", "a")}
    # Failures to acquire drop out of every comparison.
    assert len(protocol.build(ds, usable - {"c-probe"}, "all")) == 6


def test_pitch_bias_separates_camera_height():
    rng = np.random.default_rng(0)
    subjects = np.repeat(np.arange(40), 5)
    elev = rng.uniform(-10, 10, subjects.size)
    true_bias = -6.0
    p = true_bias - 1.0 * elev + rng.normal(0, 1.0, subjects.size)
    r = pitch.estimate(subjects, p, elev, n_boot=300)
    assert r["biasCi95"][0] <= true_bias <= r["biasCi95"][1]
    assert r["slopePerElevationDeg"] == pytest.approx(-1.0, abs=0.1)
    assert not r["confoundedWithCameraHeight"]
    r0 = pitch.estimate(subjects, p, None, n_boot=300)
    assert r0["confoundedWithCameraHeight"] and r0["method"] == "median"


def test_cohen_kappa():
    from facelab.eval import metrics

    assert metrics.cohen_kappa(["pass", "fail"] * 5, ["pass", "fail"] * 5) == 1.0
    # 80% raw agreement, balanced marginals -> kappa 0.6
    a = ["pass"] * 5 + ["fail"] * 5
    b = ["pass"] * 4 + ["fail"] + ["fail"] * 4 + ["pass"]
    assert metrics.cohen_kappa(a, b) == pytest.approx(0.6)
    assert metrics.cohen_kappa([], []) is None


def test_quality_uses_agreed_labels_and_pools_disagreements(tmp_path):
    quality_summary = pytest.importorskip("facelab.eval.run", reason="needs lab/.venv-mp").quality_summary

    S = dataset.Sample
    samples = [
        S("x1", "a", tmp_path, "probe", "selfie", labels={"EYES_OPEN": "pass"}, labels2={"EYES_OPEN": "pass"}),
        S("x2", "a", tmp_path, "probe", "selfie", labels={"EYES_OPEN": "fail"}, labels2={"EYES_OPEN": "pass"}),
        S("x3", "b", tmp_path, "probe", "selfie", labels={"EYES_OPEN": "fail"}),
    ]
    preds = {"x1": {"EYES_OPEN": "fail"}, "x2": {"EYES_OPEN": "pass"}, "x3": {"EYES_OPEN": "fail"}}
    q = quality_summary(samples, preds)["EYES_OPEN"]
    assert q["agreedSamples"] == 2 and q["doubleAnnotated"] == 2
    assert q["ambiguous"]["samples"] == 1 and q["ambiguous"]["predicted"]["pass"] == 1
    assert q["matrix"]["pass"]["fail"] == 1 and q["matrix"]["fail"]["fail"] == 1
