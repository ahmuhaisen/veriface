import { describe, expect, it } from 'vitest';
import {
  CROWN_EXTENSION,
  estimateHeadBox,
  eyeLineRoll,
  faceGeometry,
  headInFrame,
  headLargerThanFrame,
  IRIS_CENTER_A,
  IRIS_CENTER_B,
  worstSide,
} from '../src/measure/face-landmarks';
import { foldHalfTurn } from '../src/measure/geometry';
import { syntheticMesh } from './helpers';

const W = 1280;
const H = 720;

describe('face geometry from the 478-point mesh', () => {
  it('measures the inter-eye distance in full-frame pixels', () => {
    const mesh = syntheticMesh({ width: W, height: H, eyeMidpoint: { x: 640, y: 300 }, interEyeDistance: 120 });
    const g = faceGeometry(mesh, W, H);
    expect(g?.interEyeDistance).toBeCloseTo(120, 6);
    expect(g?.eyeMidpoint.x).toBeCloseTo(640, 6);
    expect(g?.eyeMidpoint.y).toBeCloseTo(300, 6);
  });

  it('scales normalised landmarks by width and height separately (non-square frames)', () => {
    const mesh = syntheticMesh({ width: W, height: H, eyeMidpoint: { x: 640, y: 360 }, interEyeDistance: 100, rollDegrees: 90 });
    // A vertical eye line: 100 px tall in a 720 px high frame.
    expect(faceGeometry(mesh, W, H)?.interEyeDistance).toBeCloseTo(100, 6);
  });

  it('roll > 0 for a face rotated clockwise in the image', () => {
    const mesh = syntheticMesh({ width: W, height: H, eyeMidpoint: { x: 640, y: 300 }, interEyeDistance: 120, rollDegrees: 12 });
    expect(faceGeometry(mesh, W, H)?.roll).toBeCloseTo(12, 6);
  });

  it('roll does not depend on which iris is labelled left or right', () => {
    const mesh = syntheticMesh({ width: W, height: H, eyeMidpoint: { x: 640, y: 300 }, interEyeDistance: 120, rollDegrees: -7 });
    const swapped = [...mesh];
    const a = mesh[IRIS_CENTER_A];
    const b = mesh[IRIS_CENTER_B];
    if (a === undefined || b === undefined) throw new Error('mesh too short');
    swapped[IRIS_CENTER_A] = b;
    swapped[IRIS_CENTER_B] = a;
    expect(faceGeometry(swapped, W, H)?.roll).toBeCloseTo(-7, 6);
    expect(eyeLineRoll({ a: { x: 0, y: 0 }, b: { x: -10, y: 0 } })).toBeCloseTo(0, 9);
  });

  it('folds angles into (-90, 90]', () => {
    expect(foldHalfTurn(180)).toBeCloseTo(0, 9);
    expect(foldHalfTurn(170)).toBeCloseTo(-10, 9);
    expect(foldHalfTurn(-170)).toBeCloseTo(10, 9);
    expect(foldHalfTurn(90)).toBe(90);
    expect(foldHalfTurn(-90)).toBe(90);
  });

  it('returns null without iris points', () => {
    expect(faceGeometry([{ x: 0.5, y: 0.5 }], W, H)).toBeNull();
  });
});

describe('head box and HEAD_IN_FRAME', () => {
  it('extends the mesh up to the crown', () => {
    const points = [
      { x: 500, y: 200 },
      { x: 700, y: 200 },
      { x: 500, y: 500 },
      { x: 700, y: 500 },
    ];
    const head = estimateHeadBox(points, { x: 600, y: 300 }, 0);
    expect(head?.box.y).toBeCloseTo(200 - CROWN_EXTENSION * 300, 6);
    expect(head?.headHeight).toBeCloseTo(300 * (1 + CROWN_EXTENSION), 6);
    expect(head?.box.x).toBeLessThan(500);
    expect(head?.box.x).toBeGreaterThan(400);
  });

  it('is positive inside the frame and negative when the head is cut off', () => {
    const inside = syntheticMesh({ width: W, height: H, eyeMidpoint: { x: 640, y: 330 }, interEyeDistance: 110 });
    const g = faceGeometry(inside, W, H);
    if (!g?.head) throw new Error('no head box');
    expect(headInFrame(g.head, W, H)).toBeGreaterThan(0);

    const high = syntheticMesh({ width: W, height: H, eyeMidpoint: { x: 640, y: 150 }, interEyeDistance: 110 });
    const h = faceGeometry(high, W, H);
    if (!h?.head) throw new Error('no head box');
    expect(headInFrame(h.head, W, H)).toBeLessThan(0);
    expect(worstSide(h.head.box, W, H)).toBe('top');
  });

  it('detects a head larger than the frame', () => {
    expect(headLargerThanFrame({ x: 0, y: -10, width: 500, height: 800 }, W, H)).toBe(true);
    expect(headLargerThanFrame({ x: 100, y: -10, width: 500, height: 700 }, W, H)).toBe(false);
  });
});
