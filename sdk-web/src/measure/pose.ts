import { CANONICAL_FACE, type CanonicalFace, type Vec3 } from './canonical-face';
import { RAD_TO_DEG } from './geometry';
import { fitRotation, type Matrix3 } from './procrustes';

export interface HeadPose {
  /** Degrees; > 0 means the face points towards image-right (checks.yaml HEAD_POSE_YAW). */
  readonly yaw: number;
  /** Degrees; > 0 means the face points up (checks.yaml HEAD_POSE_PITCH). */
  readonly pitch: number;
  /** Degrees; > 0 means the face is rotated clockwise in the image (checks.yaml HEAD_POSE_ROLL). */
  readonly roll: number;
}

/** A face landmark in image pixels: x right, y down, z the landmarker's depth in the same scale. */
export interface PixelLandmark {
  readonly x: number;
  readonly y: number;
  /** Depth relative to the face, in pixels; smaller (negative) is closer to the camera. */
  readonly z: number;
}

/** A MediaPipe landmark: x, y normalised by image width and height; z on the same scale as x. */
export interface NormalizedLandmark3 {
  readonly x: number;
  readonly y: number;
  readonly z: number;
}

/** MediaPipe landmarks to pixels. z is scaled by the width, like x, as MediaPipe defines it. */
export function landmarksToPixels(
  landmarks: readonly NormalizedLandmark3[],
  width: number,
  height: number,
): PixelLandmark[] {
  return landmarks.map((p) => ({ x: p.x * width, y: p.y * height, z: p.z * width }));
}

/**
 * Head pose per checks.yaml `definitions.pose`: orientation relative to the camera-to-face line of
 * sight, from a weighted, scaled-orthographic (weak-perspective) rigid Procrustes fit of the 3-D
 * landmarks to the canonical face model, using its 33 landmark weights.
 *
 * - Landmark frame for the fit: X = x, Y = -y, Z = -z (pixels), so x right, y up, z towards the
 *   camera, like the canonical model frame.
 * - Decomposition: R = Rz(c) Ry(b) Rx(a), intrinsic; yaw = +b, pitch = -a, roll = -c.
 *
 * No camera field of view is involved, so the result does not depend on where the face sits in the
 * frame. Landmarks must come from an UN-mirrored frame (a mirror flips the sign of yaw and roll).
 * Returns null when a basis landmark is missing or not finite, or the fit is degenerate.
 */
export function poseFromLandmarks(points: readonly PixelLandmark[], face: CanonicalFace = CANONICAL_FACE): HeadPose | null {
  const model: Vec3[] = [];
  const observed: Vec3[] = [];
  const weights: number[] = [];
  for (const { landmark, weight } of face.basis) {
    const p = points[landmark];
    const c = face.vertices[landmark];
    if (p === undefined || c === undefined) return null;
    if (!Number.isFinite(p.x) || !Number.isFinite(p.y) || !Number.isFinite(p.z)) return null;
    model.push(c);
    observed.push([p.x, -p.y, -p.z]);
    weights.push(weight);
  }
  const rotation = fitRotation(model, observed, weights);
  return rotation === null ? null : poseFromRotation(rotation);
}

/**
 * Yaw, pitch and roll of a rotation in the canonical frame: R = Rz(c) Ry(b) Rx(a) (intrinsic z-y-x),
 * yaw = +b, pitch = -a, roll = -c. Identical to the lab reference (`lab/src/facelab/pose.py`),
 * including its gimbal-lock rule.
 */
export function poseFromRotation(r: Matrix3): HeadPose | null {
  const at = (row: 0 | 1 | 2, col: 0 | 1 | 2): number => r[row][col];
  const b = Math.asin(Math.max(-1, Math.min(1, -at(2, 0))));
  let a: number;
  let c: number;
  if (Math.abs(Math.cos(b)) < 1e-9) {
    a = Math.atan2(-at(1, 2), at(1, 1));
    c = 0;
  } else {
    a = Math.atan2(at(2, 1), at(2, 2));
    c = Math.atan2(at(1, 0), at(0, 0));
  }
  const yaw = b * RAD_TO_DEG;
  const pitch = -a * RAD_TO_DEG;
  const roll = -c * RAD_TO_DEG;
  if (!Number.isFinite(yaw) || !Number.isFinite(pitch) || !Number.isFinite(roll)) return null;
  return { yaw, pitch, roll };
}

/** The rotation of a pose: R = Rz(-roll) Ry(yaw) Rx(-pitch). Inverse of `poseFromRotation`; for tests. */
export function rotationFromPose(pose: HeadPose): Matrix3 {
  const a = -pose.pitch / RAD_TO_DEG;
  const b = pose.yaw / RAD_TO_DEG;
  const c = -pose.roll / RAD_TO_DEG;
  const rx: Matrix3 = [
    [1, 0, 0],
    [0, Math.cos(a), -Math.sin(a)],
    [0, Math.sin(a), Math.cos(a)],
  ];
  const ry: Matrix3 = [
    [Math.cos(b), 0, Math.sin(b)],
    [0, 1, 0],
    [-Math.sin(b), 0, Math.cos(b)],
  ];
  const rz: Matrix3 = [
    [Math.cos(c), -Math.sin(c), 0],
    [Math.sin(c), Math.cos(c), 0],
    [0, 0, 1],
  ];
  return multiply(rz, multiply(ry, rx));
}

export function multiply(a: Matrix3, b: Matrix3): Matrix3 {
  const cell = (i: 0 | 1 | 2, j: 0 | 1 | 2): number => a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
  return [
    [cell(0, 0), cell(0, 1), cell(0, 2)],
    [cell(1, 0), cell(1, 1), cell(1, 2)],
    [cell(2, 0), cell(2, 1), cell(2, 2)],
  ];
}

/** Applies a rotation to a vector. */
export function rotate(r: Matrix3, v: Vec3): Vec3 {
  return [
    r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2],
    r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2],
    r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2],
  ];
}
