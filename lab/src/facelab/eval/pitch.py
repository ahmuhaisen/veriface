"""Pitch-offset study: separate the model's constant pitch bias from camera height.

Data: samples labelled ``frontal: true``, meaning the subject's HEAD IS LEVEL (Frankfort plane horizontal),
looking at a fixation mark at EYE height, while the camera is placed at a recorded elevation relative to eye level
(``cameraElevationDeg``, + = camera above the eyes). Do NOT ask subjects to look into the lens: they would turn
their head towards the camera, making head pose relative to the camera ~0 at every elevation (slope ~0), and people
often move only their eyes, so the label becomes unreliable.

Model:  pitch_raw_i = bias + k * cameraElevationDeg_i + e_i

  - With camera elevation recorded for (some) samples, fit OLS on those samples; ``bias`` is the intercept, the
    model's offset at zero elevation. k should be close to -1 (a camera above eye level makes a frontal face look
    down relative to the optical axis); a k far from -1 signals a protocol or labelling problem.
  - Without elevation, report the median pitch only, and say that it confounds model bias with the dataset's
    typical camera height.

Uncertainty: 95% percentile CI from a SUBJECT-level bootstrap (several images per subject are not independent).
The same is reported for yaw and roll, whose bias should be ~0 (a sanity check).
"""

from __future__ import annotations

import numpy as np


def _fit(pitch: np.ndarray, elev: np.ndarray | None) -> tuple[float, float | None]:
    if elev is None:
        return float(np.median(pitch)), None
    x = np.column_stack([np.ones_like(elev), elev])
    coef, *_ = np.linalg.lstsq(x, pitch, rcond=None)
    return float(coef[0]), float(coef[1])


def estimate(
    subjects: np.ndarray,
    pitch: np.ndarray,
    elevation: np.ndarray | None = None,
    n_boot: int = 1000,
    seed: int = 0,
) -> dict:
    subjects = np.asarray(subjects)
    pitch = np.asarray(pitch, np.float64)
    if elevation is not None:
        elevation = np.asarray(elevation, np.float64)
        has = ~np.isnan(elevation)
        if has.sum() < 3 or np.ptp(elevation[has]) == 0:
            elevation = None  # not enough spread to separate camera height from bias
        else:
            subjects, pitch, elevation = subjects[has], pitch[has], elevation[has]
    bias, slope = _fit(pitch, elevation)
    uniq = np.unique(subjects)
    rng = np.random.default_rng(seed)
    idx_by_subject = {s: np.flatnonzero(subjects == s) for s in uniq}
    boots_b, boots_k = [], []
    for _ in range(n_boot):
        pick = np.concatenate([idx_by_subject[s] for s in rng.choice(uniq, len(uniq))])
        b, k = _fit(pitch[pick], None if elevation is None else elevation[pick])
        boots_b.append(b)
        if k is not None:
            boots_k.append(k)
    out = {
        "method": "OLS intercept (bias) + slope on camera elevation" if elevation is not None else "median",
        "samples": int(len(pitch)),
        "subjects": int(len(uniq)),
        "biasDeg": round(bias, 2),
        "biasCi95": [round(float(v), 2) for v in np.percentile(boots_b, [2.5, 97.5])],
        "confoundedWithCameraHeight": elevation is None,
    }
    if slope is not None:
        out["slopePerElevationDeg"] = round(slope, 3)
        out["slopeCi95"] = [round(float(v), 3) for v in np.percentile(boots_k, [2.5, 97.5])]
    return out
