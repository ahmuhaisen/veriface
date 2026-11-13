"""Geometry and measure references (pose, alignment, anchors, luma). No MediaPipe needed: runs in both venvs."""

from __future__ import annotations

import math

import numpy as np
import pytest

from facelab import align, pose
from facelab.ref import blazeface


@pytest.mark.parametrize("yaw,pitch,roll", [(0, 0, 0), (20, -10, 5), (-35, 15, -12), (5, 30, 80)])
def test_pose_round_trip(yaw, pitch, roll):
    got = pose.pose_from_matrix(pose.matrix_from_pose(yaw, pitch, roll))
    assert got["yaw"] == pytest.approx(yaw, abs=1e-9)
    assert got["pitch"] == pytest.approx(pitch, abs=1e-9)
    assert got["roll"] == pytest.approx(roll, abs=1e-9)


def test_pose_signs_follow_frame():
    # Face turned towards image-right: forward (+z) rotates towards +x, a positive rotation about +y.
    t = math.radians(10)
    ry = np.array([[math.cos(t), 0, math.sin(t)], [0, 1, 0], [-math.sin(t), 0, math.cos(t)]])
    assert pose.pose_from_matrix(ry)["yaw"] == pytest.approx(10)
    # Face pointing up: forward (+z) rotates towards +y (y up), a negative rotation about +x.
    rx = np.array([[1, 0, 0], [0, math.cos(-t), -math.sin(-t)], [0, math.sin(-t), math.cos(-t)]])
    assert pose.pose_from_matrix(rx)["pitch"] == pytest.approx(10)


def test_umeyama_recovers_similarity():
    rng = np.random.default_rng(0)
    src = rng.uniform(0, 100, (5, 2))
    a, s = math.radians(17), 1.7
    m = np.array([[s * math.cos(a), -s * math.sin(a), 4.0], [s * math.sin(a), s * math.cos(a), -9.0]])
    dst = src @ m[:, :2].T + m[:, 2]
    assert np.allclose(align.umeyama_similarity(src, dst), m, atol=1e-9)


def test_anchor_counts_match_manifest():
    assert len(blazeface.anchors("short")) == 896
    assert len(blazeface.anchors("full")) == 2304


def test_weak_perspective_recovers_known_pose():
    verts, _ = pose.canonical()
    rot = pose.matrix_from_pose(yaw=12.0, pitch=-7.0, roll=5.0)
    obs = 9.5 * verts @ rot.T + np.array([300.0, -400.0, 20.0])  # X, Y up, Z toward camera
    lm = np.zeros((478, 3))
    lm[:468] = np.column_stack([obs[:, 0], -obs[:, 1], -obs[:, 2]])  # back to image px (y down, z MediaPipe)
    got = pose.fit_weak_perspective(lm)
    assert got["yaw"] == pytest.approx(12.0, abs=1e-6)
    assert got["pitch"] == pytest.approx(-7.0, abs=1e-6)
    assert got["roll"] == pytest.approx(5.0, abs=1e-6)
    assert got["scale"] == pytest.approx(9.5)
    # Translation in the image (face position) does not change the pose.
    lm2 = lm.copy()
    lm2[:, :2] += [137.0, -512.0]
    assert pose.fit_weak_perspective(lm2)["pitch"] == pytest.approx(-7.0, abs=1e-6)


def test_ofiq_luma_matches_ofiq_formula():
    from facelab import measures

    rgb = np.array([[[0, 0, 0], [255, 255, 255], [128, 128, 128], [255, 0, 0], [0, 255, 0]]], np.uint8)
    y = measures.ofiq_luma(rgb)[0]
    assert y[0] == 0 and y[1] == 255
    assert y[2] == 55  # mid-grey sRGB 128 is ~21.6% linear luminance
    assert y[3] == 54 and y[4] == 182  # BT.709 weights on linear R / G: floor(0.2126*255+.5), floor(0.7152*255+.5)
    # Levels are on the linear scale: sRGB greys <= 89 are under-exposed (L <= 25), >= 252 over (L >= 247)
    g = measures.ofiq_luma(np.array([[[89] * 3, [90] * 3, [251] * 3, [252] * 3]], np.uint8))[0]
    assert g[0] <= measures.UNDER_EXPOSURE_LEVEL < g[1]
    assert g[2] < measures.OVER_EXPOSURE_LEVEL <= g[3]


def test_illumination_on_region():
    from facelab import measures

    rgb = np.zeros((10, 10, 3), np.uint8)
    rgb[:, 5:] = 255
    region = np.zeros((10, 10), bool)
    region[:, 3:7] = True  # 2 black columns + 2 white columns
    r = measures.illumination(rgb, region)
    assert r["regionPixels"] == 40
    assert r["UNDER_EXPOSURE"] == 0.5 and r["OVER_EXPOSURE"] == 0.5 and r["LUMINANCE_MEAN"] == 0.5
    assert measures.illumination(rgb, np.zeros((10, 10), bool))["LUMINANCE_MEAN"] is None
