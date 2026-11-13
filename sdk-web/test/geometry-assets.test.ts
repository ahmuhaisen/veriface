import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { FaceLandmarker } from '@mediapipe/tasks-vision';
import { describe, expect, it } from 'vitest';
import { CANONICAL_FACE, CANONICAL_FACE_SHA256 } from '../src/measure/canonical-face';
import {
  FACE_OVAL_LOOP,
  faceSkinPolygons,
  LEFT_EYE_LOOP,
  LEFT_EYEBROW_LOOP,
  LIPS_OUTER_LOOP,
  polygonMask,
  RIGHT_EYE_LOOP,
  RIGHT_EYEBROW_LOOP,
} from '../src/measure/face-regions';
import { OVER_EXPOSURE_LEVEL, UNDER_EXPOSURE_LEVEL } from '../src/measure/pixels';
import { rotate, rotationFromPose } from '../src/measure/pose';
import { fitRotation, quaternionToMatrix, symmetricEigen } from '../src/measure/procrustes';
import { readRepoYaml } from './helpers';

const repo = (path: string) => fileURLToPath(new URL(`../../${path}`, import.meta.url));
const sha256 = (path: string) => createHash('sha256').update(readFileSync(path)).digest('hex');

describe('canonical face asset (mp-canonical-face, Apache-2.0)', () => {
  const copy = fileURLToPath(new URL('../src/assets/mp-canonical-face.json', import.meta.url));

  it('is the file recorded in models/manifest.json', () => {
    expect(sha256(copy)).toBe(CANONICAL_FACE_SHA256);
    const manifestPath = repo('models/manifest.json');
    if (existsSync(manifestPath)) {
      const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as { assets?: { id: string; sha256: string }[] };
      expect(manifest.assets?.find((a) => a.id === 'mp-canonical-face')?.sha256).toBe(CANONICAL_FACE_SHA256);
    }
    if (existsSync(repo('models/mp-canonical-face.json'))) expect(sha256(repo('models/mp-canonical-face.json'))).toBe(sha256(copy));
  });

  it('has 468 vertices and 33 weighted Procrustes landmarks', () => {
    expect(CANONICAL_FACE.vertices).toHaveLength(468);
    expect(CANONICAL_FACE.basis).toHaveLength(33);
    expect(CANONICAL_FACE.basis.every((b) => b.weight > 0)).toBe(true);
  });
});

describe('landmark region loops', () => {
  const edges = (connections: { start: number; end: number }[]) =>
    new Set(connections.flatMap(({ start, end }) => [`${start}-${end}`, `${end}-${start}`]));
  const loopEdges = (loop: readonly number[]) => loop.map((v, i) => `${v}-${loop[(i + 1) % loop.length] ?? v}`);

  it.each([
    ['face oval', FACE_OVAL_LOOP, FaceLandmarker.FACE_LANDMARKS_FACE_OVAL, 0],
    ['left eye', LEFT_EYE_LOOP, FaceLandmarker.FACE_LANDMARKS_LEFT_EYE, 0],
    ['right eye', RIGHT_EYE_LOOP, FaceLandmarker.FACE_LANDMARKS_RIGHT_EYE, 0],
    // Eyebrows are two open chains in MediaPipe; the loop adds the two closing edges.
    ['left eyebrow', LEFT_EYEBROW_LOOP, FaceLandmarker.FACE_LANDMARKS_LEFT_EYEBROW, 2],
    ['right eyebrow', RIGHT_EYEBROW_LOOP, FaceLandmarker.FACE_LANDMARKS_RIGHT_EYEBROW, 2],
  ])('%s follows MediaPipe connections', (_name, loop, connections, closingEdges) => {
    const known = edges(connections);
    expect(loopEdges(loop).filter((e) => !known.has(e))).toHaveLength(closingEdges);
    expect(new Set(loop)).toEqual(new Set(connections.flatMap((c) => [c.start, c.end])));
  });

  it('outer lips follow MediaPipe lip connections', () => {
    const known = edges(FaceLandmarker.FACE_LANDMARKS_LIPS);
    expect(loopEdges(LIPS_OUTER_LOOP).every((e) => known.has(e))).toBe(true);
  });

  it('builds the face-skin region: oval minus eyes, eyebrows and lips', () => {
    const points = Array.from({ length: 478 }, (_, i) => ({ x: i, y: i }));
    const region = faceSkinPolygons(points);
    expect(region?.include).toHaveLength(1);
    expect(region?.exclude).toHaveLength(5);
    expect(faceSkinPolygons(points.slice(0, 100))).toBeNull();
  });
});

describe('polygon mask', () => {
  const square = (x0: number, y0: number, x1: number, y1: number) => [
    { x: x0, y: y0 },
    { x: x1, y: y0 },
    { x: x1, y: y1 },
    { x: x0, y: y1 },
  ];
  const count = (m: Uint8Array) => m.reduce((s, v) => s + v, 0);

  it('fills pixel centres inside the include polygon and clears holes', () => {
    expect(count(polygonMask(20, 20, { include: [square(2, 2, 12, 8)], exclude: [] }))).toBe(60);
    expect(count(polygonMask(20, 20, { include: [square(2, 2, 12, 8)], exclude: [square(4, 4, 6, 6)] }))).toBe(56);
    expect(count(polygonMask(20, 20, { include: [square(-5, -5, 50, 50)], exclude: [] }))).toBe(400);
  });

  it('rasterises a triangle with the expected area', () => {
    const tri = [
      { x: 0, y: 0 },
      { x: 100, y: 0 },
      { x: 0, y: 100 },
    ];
    expect(Math.abs(count(polygonMask(100, 100, { include: [tri], exclude: [] })) - 5000)).toBeLessThan(100);
  });
});

describe('weighted rotation fit (Horn, unit quaternions)', () => {
  it('finds eigenpairs of a symmetric matrix', () => {
    const m = [
      [4, 1, 0, 2],
      [1, 3, 1, 0],
      [0, 1, 2, 1],
      [2, 0, 1, 5],
    ];
    const { values, vectors } = symmetricEigen(m);
    for (let k = 0; k < 4; k++) {
      for (let i = 0; i < 4; i++) {
        const av = m[i]?.reduce((s, mij, j) => s + mij * (vectors[j]?.[k] ?? 0), 0) ?? 0;
        expect(av).toBeCloseTo((values[k] ?? 0) * (vectors[i]?.[k] ?? 0), 9);
      }
    }
  });

  it('recovers a rotation exactly from rotated, scaled and shifted points', () => {
    const r = rotationFromPose({ yaw: 33, pitch: -21, roll: 12 });
    const model = CANONICAL_FACE.basis.map((b) => CANONICAL_FACE.vertices[b.landmark] ?? ([0, 0, 0] as const));
    const observed = model.map((v) => {
      const p = rotate(r, v);
      return [7 * p[0] + 100, 7 * p[1] - 40, 7 * p[2] + 5] as const;
    });
    const fit = fitRotation(model, observed, CANONICAL_FACE.basis.map((b) => b.weight));
    for (let i = 0; i < 3; i++) for (let j = 0; j < 3; j++) expect(fit?.[i as 0]?.[j as 0]).toBeCloseTo(r[i as 0][j as 0], 9);
  });

  it('returns null for degenerate input and normalises quaternions', () => {
    expect(fitRotation([[0, 0, 0]], [[0, 0, 0]], [1])).toBeNull();
    expect(quaternionToMatrix(0, 0, 0, 0)).toBeNull();
    expect(quaternionToMatrix(2, 0, 0, 0)).toEqual([
      [1, 0, 0],
      [0, 1, 0],
      [0, 0, 1],
    ]);
  });
});

describe('contract definitions (contracts/checks.yaml)', () => {
  const definitions = (readRepoYaml('contracts/checks.yaml') as { definitions: Record<string, unknown> }).definitions;

  it('uses the contract exposure levels', () => {
    expect(UNDER_EXPOSURE_LEVEL).toBe(definitions.underExposureLevel);
    expect(OVER_EXPOSURE_LEVEL).toBe(definitions.overExposureLevel);
  });

  it('implements the contract luma (OFIQ: linear sRGB, BT.709 weights)', () => {
    const luma = String(definitions.luma);
    for (const term of ['0.2126', '0.7152', '0.0722', 'floor(Y * 255 + 0.5)']) expect(luma).toContain(term);
    expect(luma).toMatch(/sRGB/);
  });
});
