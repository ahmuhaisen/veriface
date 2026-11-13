import {
  boundingRect,
  distance,
  foldHalfTurn,
  midpoint,
  RAD_TO_DEG,
  rotateAround,
  type Point,
  type Rect,
} from './geometry';

/** Normalized landmark as returned by MediaPipe (x by image width, y by image height). */
export interface NormalizedPoint {
  readonly x: number;
  readonly y: number;
}

/**
 * Iris centres in the MediaPipe 478-point face mesh (the face_landmarker model includes iris
 * points). The measurements below never depend on which one is the left or right eye.
 */
export const IRIS_CENTER_A = 468;
export const IRIS_CENTER_B = 473;
export const FACE_MESH_POINTS = 478;

/** Head-box extensions beyond the face mesh, as fractions of the mesh's own height / width. */
export const CROWN_EXTENSION = 0.3; // mesh top is the upper forehead; the crown (with hair) is above it
export const EAR_EXTENSION = 0.08; // mesh sides stop near the tragion; ears stick out a little further

export function toPixels(p: NormalizedPoint, width: number, height: number): Point {
  return { x: p.x * width, y: p.y * height };
}

/** Eye centres of a face, in image pixels. */
export interface EyePair {
  readonly a: Point;
  readonly b: Point;
}

export function irisCenters(
  landmarks: readonly NormalizedPoint[],
  width: number,
  height: number,
): EyePair | null {
  const a = landmarks[IRIS_CENTER_A];
  const b = landmarks[IRIS_CENTER_B];
  if (a === undefined || b === undefined) return null;
  return { a: toPixels(a, width, height), b: toPixels(b, width, height) };
}

export function interEyeDistance(eyes: EyePair): number {
  return distance(eyes.a, eyes.b);
}

/**
 * In-plane rotation of the eye line in degrees, folded into (-90, 90]. Positive means the face is
 * rotated clockwise in the image (checks.yaml HEAD_POSE_ROLL convention). Folding makes the result
 * independent of which eye is which, so a left/right label swap cannot flip it.
 */
export function eyeLineRoll(eyes: EyePair): number {
  return foldHalfTurn(Math.atan2(eyes.b.y - eyes.a.y, eyes.b.x - eyes.a.x) * RAD_TO_DEG);
}

export interface HeadBox {
  /** Axis-aligned box around the estimated head (crown to chin, ear to ear), in image pixels. */
  readonly box: Rect;
  /** Estimated chin-to-crown height in pixels (in the face's own, de-rotated frame). */
  readonly headHeight: number;
}

/**
 * Estimates the head box from the face mesh: the mesh's bounding box in the face's de-rotated
 * frame, extended up to the crown and out to the ears, then rotated back into the image. This is an
 * approximation; the service uses segmentation for the crown.
 */
export function estimateHeadBox(points: readonly Point[], center: Point, rollDegrees: number): HeadBox | null {
  const angle = rollDegrees / RAD_TO_DEG;
  const aligned = points.map((p) => rotateAround(p, center, -angle));
  const mesh = boundingRect(aligned);
  if (mesh === null || mesh.height <= 0 || mesh.width <= 0) return null;
  const top = mesh.y - CROWN_EXTENSION * mesh.height;
  const left = mesh.x - EAR_EXTENSION * mesh.width;
  const right = mesh.x + mesh.width + EAR_EXTENSION * mesh.width;
  const bottom = mesh.y + mesh.height;
  const corners: Point[] = [
    { x: left, y: top },
    { x: right, y: top },
    { x: right, y: bottom },
    { x: left, y: bottom },
  ].map((p) => rotateAround(p, center, angle));
  const box = boundingRect(corners);
  if (box === null) return null;
  return { box, headHeight: bottom - top };
}

/**
 * HEAD_IN_FRAME value: the smallest margin between the head box and the image border, divided by
 * the head height. Negative when the head is cut off.
 */
export function headInFrame(head: HeadBox, width: number, height: number): number {
  const { box } = head;
  const margin = Math.min(box.x, box.y, width - (box.x + box.width), height - (box.y + box.height));
  return margin / head.headHeight;
}

/** Image side where the head box sticks out furthest, when it does. */
export type FrameSide = 'left' | 'right' | 'top' | 'bottom';

export function worstSide(box: Rect, width: number, height: number): FrameSide {
  const margins: [FrameSide, number][] = [
    ['top', box.y],
    ['right', width - (box.x + box.width)],
    ['bottom', height - (box.y + box.height)],
  ];
  let worst: [FrameSide, number] = ['left', box.x];
  for (const entry of margins) if (entry[1] < worst[1]) worst = entry;
  return worst[0];
}

/** True when the head is larger than the frame, so only moving back can fit it in. */
export function headLargerThanFrame(box: Rect, width: number, height: number): boolean {
  return box.width > width || box.height > height;
}

export interface FaceGeometry {
  readonly eyes: EyePair;
  readonly eyeMidpoint: Point;
  readonly interEyeDistance: number;
  readonly roll: number;
  readonly head: HeadBox | null;
}

/** Geometry of one face from its 478 normalized landmarks. */
export function faceGeometry(
  landmarks: readonly NormalizedPoint[],
  width: number,
  height: number,
): FaceGeometry | null {
  const eyes = irisCenters(landmarks, width, height);
  if (eyes === null) return null;
  const eyeMidpoint = midpoint(eyes.a, eyes.b);
  const roll = eyeLineRoll(eyes);
  const points = landmarks.map((p) => toPixels(p, width, height));
  return {
    eyes,
    eyeMidpoint,
    interEyeDistance: interEyeDistance(eyes),
    roll,
    head: estimateHeadBox(points, eyeMidpoint, roll),
  };
}
