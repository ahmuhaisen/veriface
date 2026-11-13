import { describe, expect, it } from 'vitest';
import { resolveSelfieGuidanceConfig } from '../src/capture/selfie';
import { evaluateSelfie, selfieCheckValues } from '../src/guidance/selfie';
import { selfieFace, selfieMeasurements } from './helpers';

const config = resolveSelfieGuidanceConfig({});
const codes = (m: Parameters<typeof evaluateSelfie>[0], c = config) => evaluateSelfie(m, c).issues.map((i) => i.code);

describe('selfie guidance', () => {
  it('passes a good, centred, frontal face', () => {
    const guidance = evaluateSelfie(selfieMeasurements(), config);
    expect(guidance.passed).toBe(true);
    expect(guidance.issues).toEqual([]);
    expect(guidance.checks.filter((c) => c.status !== 'pass').map((c) => c.id)).toEqual([]);
  });

  it('NO_FACE when no face is detected, and nothing else', () => {
    const guidance = evaluateSelfie(selfieMeasurements(null), config);
    expect(guidance.issues.map((i) => i.code)).toEqual(['NO_FACE']);
    expect(guidance.issues[0]?.checkId).toBe('FACE_PRESENT');
    expect(guidance.checks.find((c) => c.id === 'HEAD_POSE_YAW')?.status).toBe('undetermined');
  });

  it('MULTIPLE_FACES when two counted faces are visible, ranked first', () => {
    const m = selfieMeasurements(selfieFace({ yaw: 30 }), { faceInterEyeDistances: [150, 90] });
    expect(codes(m)[0]).toBe('MULTIPLE_FACES');
    // Geometry and pose of "the" face are meaningless with two faces (prerequisite SINGLE_FACE).
    expect(codes(m)).not.toContain('YAW_OUT_OF_RANGE');
  });

  it('ignores small background faces (faceCounting.minInterEyeDistanceRatio x width)', () => {
    // 0.02 x 1280 = 25.6 px: a 20 px face in the background does not count.
    const m = selfieMeasurements(selfieFace(), { faceInterEyeDistances: [150, 20] });
    expect(selfieCheckValues(m, 0.02).SINGLE_FACE).toBe(1);
    expect(codes(m)).toEqual([]);
  });

  it('TOO_FAR below the profile inter-eye minimum (60 px)', () => {
    const issues = evaluateSelfie(selfieMeasurements(selfieFace({ interEyeDistance: 50 })), config).issues;
    expect(issues[0]).toMatchObject({ code: 'TOO_FAR', checkId: 'INTER_EYE_DISTANCE', value: 50, min: 60 });
  });

  it('TOO_CLOSE above the SDK bound (inter-eye distance / shorter side)', () => {
    // 0.35 x 720 = 252 px.
    const issues = evaluateSelfie(selfieMeasurements(selfieFace({ interEyeDistance: 260 })), config).issues;
    expect(issues[0]).toMatchObject({ code: 'TOO_CLOSE', checkId: 'INTER_EYE_DISTANCE', sdkOnly: true });
    expect(issues[0]?.max).toBeCloseTo(252, 9);
    // Exactly on the bound is still fine (inclusive), despite floating-point rounding.
    expect(codes(selfieMeasurements(selfieFace({ interEyeDistance: 252 })))).toEqual([]);
    expect(codes(selfieMeasurements(selfieFace({ interEyeDistance: 252.01 })))).toEqual(['TOO_CLOSE']);
  });

  it('no TOO_CLOSE when the SDK bound is disabled', () => {
    const relaxed = resolveSelfieGuidanceConfig({ maxInterEyeRatio: null });
    expect(codes(selfieMeasurements(selfieFace({ interEyeDistance: 400 })), relaxed)).toEqual([]);
  });

  it('HEAD_OUT_OF_FRAME with the side where the head is cut off', () => {
    const face = selfieFace({ headBox: { x: 1000, y: 80, width: 400, height: 560 }, headInFrame: -120 / 560 });
    const issue = evaluateSelfie(selfieMeasurements(face), config).issues[0];
    expect(issue).toMatchObject({ code: 'HEAD_OUT_OF_FRAME', checkId: 'HEAD_IN_FRAME', direction: 'right' });
  });

  it('TOO_CLOSE when the head is larger than the frame', () => {
    const face = selfieFace({ headBox: { x: 340, y: -60, width: 600, height: 840 }, headInFrame: -60 / 840 });
    const issue = evaluateSelfie(selfieMeasurements(face), config).issues[0];
    expect(issue).toMatchObject({ code: 'TOO_CLOSE', checkId: 'HEAD_IN_FRAME' });
  });

  it.each([
    [{ yaw: 20 }, 'YAW_OUT_OF_RANGE', 'right'],
    [{ yaw: -20 }, 'YAW_OUT_OF_RANGE', 'left'],
    [{ pitch: 16 }, 'PITCH_OUT_OF_RANGE', 'up'],
    [{ pitch: -16 }, 'PITCH_OUT_OF_RANGE', 'down'],
    [{ roll: 12 }, 'ROLL_OUT_OF_RANGE', 'clockwise'],
    [{ roll: -12 }, 'ROLL_OUT_OF_RANGE', 'counterclockwise'],
  ] as const)('pose %o -> %s (%s)', (pose, code, direction) => {
    const issue = evaluateSelfie(selfieMeasurements(selfieFace(pose)), config).issues[0];
    expect(issue).toMatchObject({ code, direction });
  });

  it('accepts pose exactly on the bounds (inclusive)', () => {
    expect(codes(selfieMeasurements(selfieFace({ yaw: 15, pitch: -15, roll: 10 })))).toEqual([]);
  });

  it.each([
    [{ eyesOpen: 0.3 }, 'EYES_CLOSED', 'EYES_OPEN'],
    [{ luminanceMean: 0.05 }, 'TOO_DARK', 'LUMINANCE_MEAN'],
    [{ luminanceMean: 0.95 }, 'TOO_BRIGHT', 'LUMINANCE_MEAN'],
    [{ underExposure: 0.8 }, 'TOO_DARK', 'UNDER_EXPOSURE'],
    [{ overExposure: 0.2 }, 'TOO_BRIGHT', 'OVER_EXPOSURE'],
    [{ sharpness: 0.2 }, 'BLURRY', 'SHARPNESS'],
  ] as const)('%o -> %s from %s', (face, code, checkId) => {
    const issues = evaluateSelfie(selfieMeasurements(selfieFace(face)), config).issues;
    expect(issues).toHaveLength(1);
    expect(issues[0]).toMatchObject({ code, checkId });
  });

  it('LOW_RESOLUTION when the camera delivers less than the profile minimum', () => {
    const m = selfieMeasurements(selfieFace({ interEyeDistance: 80 }), { frameWidth: 640, frameHeight: 360 });
    expect(codes(m)).toContain('LOW_RESOLUTION');
  });

  it('orders several issues root cause first (catalogue priority)', () => {
    const face = selfieFace({ sharpness: 0.1, luminanceMean: 0.05, yaw: 25, eyesOpen: 0.1 });
    expect(codes(selfieMeasurements(face))).toEqual(['YAW_OUT_OF_RANGE', 'EYES_CLOSED', 'TOO_DARK', 'BLURRY']);
  });

  it('turns warning-severity failures into advisories that never block', () => {
    const lenient = resolveSelfieGuidanceConfig({ checks: { SHARPNESS: { severity: 'warning' } } });
    const guidance = evaluateSelfie(selfieMeasurements(selfieFace({ sharpness: 0.1 })), lenient);
    expect(guidance.passed).toBe(true);
    expect(guidance.issues).toEqual([]);
    expect(guidance.advisories.map((a) => a.code)).toEqual(['BLURRY']);
  });

  it('does not block on checks it could not assess (the service re-checks)', () => {
    const face = selfieFace({ yaw: null, pitch: null, eyesOpen: null, sharpness: null });
    const guidance = evaluateSelfie(selfieMeasurements(face), config);
    expect(guidance.passed).toBe(true);
    expect(guidance.checks.find((c) => c.id === 'HEAD_POSE_YAW')?.status).toBe('undetermined');
  });

  it('honours disabled checks', () => {
    const noYaw = resolveSelfieGuidanceConfig({ checks: { HEAD_POSE_YAW: { enabled: false } } });
    expect(codes(selfieMeasurements(selfieFace({ yaw: 40 })), noYaw)).toEqual([]);
  });

  it('applies the thresholds of a stricter profile', () => {
    const strict = resolveSelfieGuidanceConfig({
      profile: {
        selfie: {
          checks: {
            FACE_PRESENT: { enabled: true, min: 1 },
            HEAD_POSE_YAW: { enabled: true, min: -5, max: 5 },
          },
        },
      },
    });
    expect(codes(selfieMeasurements(selfieFace({ yaw: 6 })), strict)).toEqual(['YAW_OUT_OF_RANGE']);
    // Unlisted checks are skipped, as in the service.
    expect(codes(selfieMeasurements(selfieFace({ sharpness: 0 })), strict)).toEqual([]);
  });
});
