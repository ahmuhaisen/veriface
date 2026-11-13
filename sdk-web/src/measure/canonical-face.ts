import asset from '../assets/mp-canonical-face.json';

/** A 3-D point. */
export type Vec3 = readonly [number, number, number];

/**
 * MediaPipe's canonical face model (asset `mp-canonical-face`, Apache-2.0): 468 vertices in cm, in
 * the face-geometry metric frame (x right, y up, z towards the camera; vertex i = landmark i), plus
 * the 33 weighted landmarks MediaPipe uses for its Procrustes fit.
 *
 * `src/assets/mp-canonical-face.json` is a verbatim copy of `models/mp-canonical-face.json`; a test
 * pins it to the SHA-256 recorded in `models/manifest.json`.
 */
export interface CanonicalFace {
  readonly vertices: readonly Vec3[];
  readonly basis: readonly { readonly landmark: number; readonly weight: number }[];
}

export const CANONICAL_FACE_SHA256 = 'e11bd339c3f18bf30b59683abc32f7f1700ddf3e72c5a062c4aa068f64728a3b';

function load(raw: unknown): CanonicalFace {
  const data = raw as { vertices?: unknown; procrustesLandmarkBasis?: unknown };
  const vertices = data.vertices;
  const basis = data.procrustesLandmarkBasis;
  if (!Array.isArray(vertices) || vertices.length !== 468 || !Array.isArray(basis) || basis.length !== 33) {
    throw new Error('mp-canonical-face: expected 468 vertices and 33 Procrustes landmarks');
  }
  const isVec3 = (v: unknown): v is Vec3 =>
    Array.isArray(v) && v.length === 3 && v.every((n) => typeof n === 'number' && Number.isFinite(n));
  const isBasis = (b: unknown): b is { landmark: number; weight: number } => {
    const entry = b as { landmark?: unknown; weight?: unknown };
    return (
      typeof entry.landmark === 'number' &&
      Number.isInteger(entry.landmark) &&
      entry.landmark >= 0 &&
      entry.landmark < 468 &&
      typeof entry.weight === 'number' &&
      entry.weight > 0
    );
  };
  if (!vertices.every(isVec3) || !basis.every(isBasis)) throw new Error('mp-canonical-face: malformed data');
  return { vertices, basis };
}

export const CANONICAL_FACE: CanonicalFace = load(asset);
