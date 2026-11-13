import type { Point } from './geometry';

/**
 * Closed landmark loops of the MediaPipe face mesh, built from its connection sets
 * (FACE_LANDMARKS_FACE_OVAL, _LEFT_EYE, _RIGHT_EYE, _LEFT_EYEBROW, _RIGHT_EYEBROW, _LIPS); a test
 * checks them against the constants shipped in @mediapipe/tasks-vision.
 */
export const FACE_OVAL_LOOP: readonly number[] = [
  10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150, 136,
  172, 58, 132, 93, 234, 127, 162, 21, 54, 103, 67, 109,
];
/** Lower lid 263 → 362, then the upper lid back. */
export const LEFT_EYE_LOOP: readonly number[] = [263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466];
export const RIGHT_EYE_LOOP: readonly number[] = [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246];
/** Lower edge (lateral → medial), then the upper edge back. */
export const LEFT_EYEBROW_LOOP: readonly number[] = [276, 283, 282, 295, 285, 336, 296, 334, 293, 300];
export const RIGHT_EYEBROW_LOOP: readonly number[] = [46, 53, 52, 65, 55, 107, 66, 105, 63, 70];
/** Outer lip contour: lower lip 61 → 291, then the upper lip back. */
export const LIPS_OUTER_LOOP: readonly number[] = [
  61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 409, 270, 269, 267, 0, 37, 39, 40, 185,
];

export interface RegionPolygons {
  readonly include: readonly (readonly Point[])[];
  readonly exclude: readonly (readonly Point[])[];
}

/**
 * The face-skin region of checks.yaml `definitions.faceSkinRegion` for when no segmentation is
 * available: the landmark face oval minus the eye, eyebrow and lip regions. Points are landmark
 * positions in any 2-D frame; null when a landmark is missing.
 */
export function faceSkinPolygons(points: readonly Point[]): RegionPolygons | null {
  const loop = (indices: readonly number[]): Point[] | null => {
    const out: Point[] = [];
    for (const i of indices) {
      const p = points[i];
      if (p === undefined || !Number.isFinite(p.x) || !Number.isFinite(p.y)) return null;
      out.push(p);
    }
    return out;
  };
  const oval = loop(FACE_OVAL_LOOP);
  const holes = [LEFT_EYE_LOOP, RIGHT_EYE_LOOP, LEFT_EYEBROW_LOOP, RIGHT_EYEBROW_LOOP, LIPS_OUTER_LOOP].map(loop);
  if (oval === null || holes.some((h) => h === null)) return null;
  return { include: [oval], exclude: holes.filter((h): h is Point[] => h !== null) };
}

/**
 * Rasterises polygons into a width x height mask: 1 where a pixel centre lies inside an include
 * polygon and inside no exclude polygon (even-odd rule per polygon), else 0.
 */
export function polygonMask(width: number, height: number, region: RegionPolygons): Uint8Array {
  const mask = new Uint8Array(width * height);
  const fill = (polygon: readonly Point[], value: 0 | 1) => {
    const n = polygon.length;
    if (n < 3) return;
    const xs: number[] = [];
    for (let y = 0; y < height; y++) {
      const cy = y + 0.5;
      xs.length = 0;
      for (let i = 0; i < n; i++) {
        const a = polygon[i];
        const b = polygon[(i + 1) % n];
        if (a === undefined || b === undefined) continue;
        if ((a.y <= cy && b.y > cy) || (b.y <= cy && a.y > cy)) {
          xs.push(a.x + ((cy - a.y) / (b.y - a.y)) * (b.x - a.x));
        }
      }
      xs.sort((p, q) => p - q);
      for (let k = 0; k + 1 < xs.length; k += 2) {
        // Pixels whose centre x + 0.5 lies in [x0, x1).
        const from = Math.max(0, Math.ceil((xs[k] ?? 0) - 0.5));
        const to = Math.min(width, Math.ceil((xs[k + 1] ?? 0) - 0.5));
        for (let x = from; x < to; x++) mask[y * width + x] = value;
      }
    }
  };
  for (const polygon of region.include) fill(polygon, 1);
  for (const polygon of region.exclude) fill(polygon, 0);
  return mask;
}
