import type { Vec3 } from './canonical-face';

/** A 3x3 matrix, row-major: m[row][col]. */
export type Matrix3 = readonly [Vec3, Vec3, Vec3];

/**
 * Weighted absolute orientation: the rotation R of the similarity transform b ≈ s R a + t that
 * minimises sum_i w_i |s R a_i + t - b_i|^2 (weighted Procrustes). Scale and translation do not change
 * the optimal rotation, so only R is returned.
 *
 * Solved in closed form with unit quaternions (B. K. P. Horn, "Closed-form solution of absolute
 * orientation using unit quaternions", JOSA A 4(4), 1987), which always yields a proper rotation.
 * Returns null for fewer than three points or a degenerate configuration.
 */
export function fitRotation(
  model: readonly Vec3[],
  observed: readonly Vec3[],
  weights: readonly number[],
): Matrix3 | null {
  const n = model.length;
  if (n < 3 || observed.length !== n || weights.length !== n) return null;
  // Weighted centroids.
  let total = 0;
  let max = 0;
  let may = 0;
  let maz = 0;
  let mbx = 0;
  let mby = 0;
  let mbz = 0;
  for (let i = 0; i < n; i++) {
    const w = weights[i] ?? 0;
    const a = model[i];
    const b = observed[i];
    if (!(w > 0) || a === undefined || b === undefined) return null;
    total += w;
    max += w * a[0];
    may += w * a[1];
    maz += w * a[2];
    mbx += w * b[0];
    mby += w * b[1];
    mbz += w * b[2];
  }
  max /= total;
  may /= total;
  maz /= total;
  mbx /= total;
  mby /= total;
  mbz /= total;
  // Weighted cross-covariance S_uv = sum w (a - mean a)_u (b - mean b)_v, model a, observed b.
  let xx = 0;
  let xy = 0;
  let xz = 0;
  let yx = 0;
  let yy = 0;
  let yz = 0;
  let zx = 0;
  let zy = 0;
  let zz = 0;
  for (let i = 0; i < n; i++) {
    const w = weights[i] ?? 0;
    const a = model[i];
    const b = observed[i];
    if (a === undefined || b === undefined) return null;
    const ax = a[0] - max;
    const ay = a[1] - may;
    const az = a[2] - maz;
    const bx = b[0] - mbx;
    const by = b[1] - mby;
    const bz = b[2] - mbz;
    xx += w * ax * bx;
    xy += w * ax * by;
    xz += w * ax * bz;
    yx += w * ay * bx;
    yy += w * ay * by;
    yz += w * ay * bz;
    zx += w * az * bx;
    zy += w * az * by;
    zz += w * az * bz;
  }
  const N = [
    [xx + yy + zz, yz - zy, zx - xz, xy - yx],
    [yz - zy, xx - yy - zz, xy + yx, zx + xz],
    [zx - xz, xy + yx, -xx + yy - zz, yz + zy],
    [xy - yx, zx + xz, yz + zy, -xx - yy + zz],
  ];
  if (!N.every((row) => row.every(Number.isFinite))) return null;
  const { values, vectors } = symmetricEigen(N);
  let best = 0;
  for (let i = 1; i < values.length; i++) if ((values[i] ?? -Infinity) > (values[best] ?? -Infinity)) best = i;
  const q = vectors.map((row) => row[best] ?? 0);
  return quaternionToMatrix(q[0] ?? 1, q[1] ?? 0, q[2] ?? 0, q[3] ?? 0);
}

/** Rotation matrix of the quaternion (w, x, y, z), normalised first. */
export function quaternionToMatrix(w: number, x: number, y: number, z: number): Matrix3 | null {
  const norm = Math.hypot(w, x, y, z);
  if (!Number.isFinite(norm) || norm < 1e-12) return null;
  const [qw, qx, qy, qz] = [w / norm, x / norm, y / norm, z / norm];
  return [
    [1 - 2 * (qy * qy + qz * qz), 2 * (qx * qy - qw * qz), 2 * (qx * qz + qw * qy)],
    [2 * (qx * qy + qw * qz), 1 - 2 * (qx * qx + qz * qz), 2 * (qy * qz - qw * qx)],
    [2 * (qx * qz - qw * qy), 2 * (qy * qz + qw * qx), 1 - 2 * (qx * qx + qy * qy)],
  ];
}

/**
 * Eigen-decomposition of a small symmetric matrix by cyclic Jacobi rotations. Returns the eigenvalues
 * and the eigenvectors as the COLUMNS of `vectors`.
 */
export function symmetricEigen(matrix: readonly (readonly number[])[]): { values: number[]; vectors: number[][] } {
  const n = matrix.length;
  const a = matrix.map((row) => [...row]);
  const v = Array.from({ length: n }, (_, i) => Array.from({ length: n }, (__, j) => (i === j ? 1 : 0)));
  const get = (m: number[][], i: number, j: number): number => m[i]?.[j] ?? 0;
  const set = (m: number[][], i: number, j: number, value: number): void => {
    const row = m[i];
    if (row) row[j] = value;
  };
  for (let sweep = 0; sweep < 100; sweep++) {
    let off = 0;
    let diagonal = 0;
    for (let i = 0; i < n; i++) {
      diagonal += get(a, i, i) ** 2;
      for (let j = i + 1; j < n; j++) off += get(a, i, j) ** 2;
    }
    if (off <= 1e-24 * Math.max(diagonal, 1e-300)) break;
    for (let p = 0; p < n; p++) {
      for (let q = p + 1; q < n; q++) {
        const apq = get(a, p, q);
        if (Math.abs(apq) < 1e-300) continue;
        const theta = (get(a, q, q) - get(a, p, p)) / (2 * apq);
        const t = (theta >= 0 ? 1 : -1) / (Math.abs(theta) + Math.sqrt(theta * theta + 1));
        const c = 1 / Math.sqrt(t * t + 1);
        const s = t * c;
        for (let k = 0; k < n; k++) {
          const akp = get(a, k, p);
          const akq = get(a, k, q);
          set(a, k, p, c * akp - s * akq);
          set(a, k, q, s * akp + c * akq);
        }
        for (let k = 0; k < n; k++) {
          const apk = get(a, p, k);
          const aqk = get(a, q, k);
          set(a, p, k, c * apk - s * aqk);
          set(a, q, k, s * apk + c * aqk);
        }
        for (let k = 0; k < n; k++) {
          const vkp = get(v, k, p);
          const vkq = get(v, k, q);
          set(v, k, p, c * vkp - s * vkq);
          set(v, k, q, s * vkp + c * vkq);
        }
      }
    }
  }
  return { values: Array.from({ length: n }, (_, i) => get(a, i, i)), vectors: v };
}
