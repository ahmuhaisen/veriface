import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { CANONICAL_FACE } from '../src/measure/canonical-face';
import {
  landmarksToPixels,
  poseFromLandmarks,
  poseFromRotation,
  rotate,
  rotationFromPose,
  type HeadPose,
  type PixelLandmark,
} from '../src/measure/pose';

/**
 * Landmarks of the canonical face in a given pose, as the landmarker reports them: image pixels
 * (x right, y down) and depth z in the same scale, smaller = closer. Weak perspective: one scale.
 */
function synthetic(pose: HeadPose, options: { scale?: number; cx?: number; cy?: number; z0?: number } = {}): PixelLandmark[] {
  const { scale = 20, cx = 640, cy = 360, z0 = 0 } = options;
  const r = rotationFromPose(pose);
  return CANONICAL_FACE.vertices.map((v) => {
    const p = rotate(r, v);
    return { x: cx + scale * p[0], y: cy - scale * p[1], z: z0 - scale * p[2] };
  });
}

function expectPose(actual: HeadPose | null, expected: HeadPose, digits = 6): void {
  expect(actual).not.toBeNull();
  expect(actual?.yaw).toBeCloseTo(expected.yaw, digits);
  expect(actual?.pitch).toBeCloseTo(expected.pitch, digits);
  expect(actual?.roll).toBeCloseTo(expected.roll, digits);
}

describe('pose decomposition (checks.yaml definitions.pose)', () => {
  it('reads the identity as a frontal face', () => {
    expectPose(poseFromRotation(rotationFromPose({ yaw: 0, pitch: 0, roll: 0 })), { yaw: 0, pitch: 0, roll: 0 });
  });

  it.each([
    { yaw: -30, pitch: 0, roll: 0 },
    { yaw: 10, pitch: -20, roll: 0 },
    { yaw: 14, pitch: 9, roll: 7 },
    { yaw: -22, pitch: 17, roll: -12 },
    { yaw: 0, pitch: 0, roll: -17 },
  ])('round-trips %o through R = Rz(-roll) Ry(yaw) Rx(-pitch)', (pose) => {
    expectPose(poseFromRotation(rotationFromPose(pose)), pose);
  });

  it('follows the contract signs: yaw towards image-right, pitch up, roll clockwise', () => {
    // Positive yaw turns the face's forward axis (+z, towards the camera) towards +x: image-right.
    expect(rotate(rotationFromPose({ yaw: 20, pitch: 0, roll: 0 }), [0, 0, 1])[0]).toBeGreaterThan(0);
    // Positive pitch tips the forward axis up (+y is up in the canonical frame).
    expect(rotate(rotationFromPose({ yaw: 0, pitch: 12, roll: 0 }), [0, 0, 1])[1]).toBeGreaterThan(0);
    // Positive roll moves the face's right axis (+x) downwards (-y): clockwise on screen.
    expect(rotate(rotationFromPose({ yaw: 0, pitch: 0, roll: 8 }), [1, 0, 0])[1]).toBeLessThan(0);
  });

  it('handles gimbal lock like the reference (everything in pitch, roll 0)', () => {
    const pose = poseFromRotation(rotationFromPose({ yaw: 90, pitch: 10, roll: 0 }));
    expect(pose?.yaw).toBeCloseTo(90, 4);
    expect(pose?.roll).toBeCloseTo(0, 9);
  });
});

describe('pose from landmarks: weak-perspective fit to the canonical face', () => {
  it.each([
    { yaw: 0, pitch: 0, roll: 0 },
    { yaw: 25, pitch: 0, roll: 0 },
    { yaw: -12, pitch: 8, roll: 4 },
    { yaw: 7, pitch: -18, roll: -9 },
    { yaw: 0, pitch: 0, roll: 30 },
  ])('recovers %o exactly from ideal landmarks', (pose) => {
    expectPose(poseFromLandmarks(synthetic(pose)), pose);
  });

  it('does not depend on where the face is in the frame, its size or its depth offset', () => {
    const pose = { yaw: 11, pitch: -6, roll: 3 };
    const reference = poseFromLandmarks(synthetic(pose));
    for (const at of [
      { cx: 60, cy: 50 },
      { cx: 1200, cy: 60 },
      { cx: 640, cy: 1240 },
      { cx: 30, cy: 700, scale: 6, z0: -500 },
      { cx: 900, cy: 400, scale: 55, z0: 80 },
    ]) {
      const moved = poseFromLandmarks(synthetic(pose, at));
      expect(moved?.yaw).toBeCloseTo(reference?.yaw ?? Number.NaN, 9);
      expect(moved?.pitch).toBeCloseTo(reference?.pitch ?? Number.NaN, 9);
      expect(moved?.roll).toBeCloseTo(reference?.roll ?? Number.NaN, 9);
    }
  });

  it('is stable under landmark noise', () => {
    const pose = { yaw: -9, pitch: 5, roll: 2 };
    let seed = 7;
    const noise = () => {
      seed = (seed * 16807) % 2147483647;
      return (seed / 2147483647 - 0.5) * 2; // +-1 px
    };
    const noisy = synthetic(pose).map((p) => ({ x: p.x + noise(), y: p.y + noise(), z: p.z + noise() }));
    const result = poseFromLandmarks(noisy);
    expect(Math.abs((result?.yaw ?? 99) - pose.yaw)).toBeLessThan(1);
    expect(Math.abs((result?.pitch ?? 99) - pose.pitch)).toBeLessThan(1);
    expect(Math.abs((result?.roll ?? 99) - pose.roll)).toBeLessThan(1);
  });

  it('converts MediaPipe landmarks with z on the x scale', () => {
    expect(landmarksToPixels([{ x: 0.5, y: 0.25, z: -0.1 }], 1280, 720)).toEqual([{ x: 640, y: 180, z: -128 }]);
  });

  it('returns null for missing or non-finite landmarks', () => {
    expect(poseFromLandmarks([])).toBeNull();
    const broken = synthetic({ yaw: 0, pitch: 0, roll: 0 });
    broken[4] = { x: Number.NaN, y: 0, z: 0 };
    expect(poseFromLandmarks(broken)).toBeNull();
  });
});

// ---------------------------------------------------------------------------------------------------
// Cross-component checks against the lab's golden outputs (read-only). Format v2 goldens carry the
// weak-perspective pose of the reference implementation (lab/src/facelab/pose.py) for real MediaPipe
// landmarks, plus invariant fixtures (horizontal flip, 15 degree rotation, same face at the top and
// the bottom of a canvas).

const goldenDir = fileURLToPath(new URL('../../testdata/golden/pipeline', import.meta.url));

interface GoldenFace {
  landmarks?: number[][];
  pose?: HeadPose;
}

interface Golden {
  formatVersion?: number;
  faces?: GoldenFace[];
  invariants?: {
    comparedWith: string;
    transform: string;
    toleranceDeg: number;
  } & Record<'yaw' | 'pitch' | 'roll', string>;
}

const goldens: Map<string, Golden> = existsSync(goldenDir)
  ? new Map(
      readdirSync(goldenDir)
        .filter((file) => file.endsWith('.json'))
        .map((file) => [file.replace('.json', ''), JSON.parse(readFileSync(`${goldenDir}/${file}`, 'utf8')) as Golden]),
    )
  : new Map<string, Golden>();

function fit(face: GoldenFace | undefined): HeadPose | null {
  const points = (face?.landmarks ?? []).map(([x, y, z]) => ({ x: x ?? Number.NaN, y: y ?? Number.NaN, z: z ?? Number.NaN }));
  return poseFromLandmarks(points);
}

const poseCases = [...goldens]
  .filter(([, golden]) => (golden.formatVersion ?? 0) >= 2)
  .flatMap(([id, golden]) =>
    (golden.faces ?? [])
      .map((face, i) => [`${id}#${i}`, face] as const)
      .filter(([, face]) => face.landmarks !== undefined && face.pose !== undefined),
  );

describe.runIf(poseCases.length > 0)('agreement with the lab reference pose (golden format v2)', () => {
  it.each(poseCases)('%s', (_id, face) => {
    const pose = fit(face);
    // Goldens are rounded to 4 decimals.
    expect(Math.abs((pose?.yaw ?? 99) - (face.pose?.yaw ?? 0))).toBeLessThan(0.002);
    expect(Math.abs((pose?.pitch ?? 99) - (face.pose?.pitch ?? 0))).toBeLessThan(0.002);
    expect(Math.abs((pose?.roll ?? 99) - (face.pose?.roll ?? 0))).toBeLessThan(0.002);
  });
});

const invariantCases = [...goldens].filter(([, golden]) => golden.invariants !== undefined);

describe.runIf(invariantCases.length > 0)('invariants on real landmarks (golden fixtures)', () => {
  it.each(invariantCases)('%s', (_id, golden) => {
    const inv = golden.invariants;
    if (inv === undefined) throw new Error('no invariants');
    const fixture = fit(golden.faces?.[0]);
    const parent = fit(goldens.get(inv.comparedWith)?.faces?.[0]);
    expect(fixture).not.toBeNull();
    expect(parent).not.toBeNull();
    for (const angle of ['yaw', 'pitch', 'roll'] as const) {
      const rule = inv[angle];
      const a = parent?.[angle] ?? Number.NaN;
      const b = fixture?.[angle] ?? Number.NaN;
      const plus = /^plus\s+(-?\d+(?:\.\d+)?)$/.exec(rule);
      const expected = rule === 'equal' ? a : rule === 'negated' ? -a : plus ? a + Number(plus[1]) : Number.NaN;
      expect(Number.isFinite(expected), `unknown invariant "${rule}"`).toBe(true);
      expect(Math.abs(b - expected), `${angle}: ${rule}`).toBeLessThanOrEqual(inv.toleranceDeg);
    }
  });
});
