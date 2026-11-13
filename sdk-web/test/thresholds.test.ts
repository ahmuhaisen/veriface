import { describe, expect, it } from 'vitest';
import { resolveDocumentGuidanceConfig } from '../src/capture/document';
import { resolveSessionOptions } from '../src/capture/session';
import { resolveSelfieGuidanceConfig } from '../src/capture/selfie';
import type { CommonCaptureOptions } from '../src/capture/types';
import { FaceCaptureError } from '../src/errors';
import {
  DEFAULT_DOCUMENT_CHECKS,
  DEFAULT_SELFIE_CHECKS,
  DEFAULT_SELFIE_STABILITY,
  resolveCheckSettings,
  resolveFaceCountingRatio,
  resolveStability,
  validateRatio,
  type CheckSetting,
} from '../src/thresholds';

function expectInvalid(fn: () => unknown): void {
  try {
    fn();
  } catch (error) {
    expect(error).toBeInstanceOf(FaceCaptureError);
    expect((error as FaceCaptureError).code).toBe('invalid-options');
    return;
  }
  throw new Error('expected an invalid-options error');
}

describe('resolveCheckSettings', () => {
  it('returns the defaults when nothing is given', () => {
    expect(resolveCheckSettings(DEFAULT_SELFIE_CHECKS)).toEqual(DEFAULT_SELFIE_CHECKS);
  });

  it('replaces the defaults with a profile section; unlisted checks are skipped', () => {
    const profile: Record<string, CheckSetting> = {
      FACE_PRESENT: { enabled: true, severity: 'error', min: 1 },
      HEAD_POSE_YAW: { enabled: true, severity: 'error', min: -5, max: 5 },
    };
    const resolved = resolveCheckSettings(DEFAULT_SELFIE_CHECKS, profile);
    expect(resolved.HEAD_POSE_YAW).toEqual({ enabled: true, severity: 'error', min: -5, max: 5 });
    expect(resolved.SHARPNESS).toBeUndefined();
    expect(Object.keys(resolved).sort()).toEqual(['FACE_PRESENT', 'HEAD_POSE_YAW']);
  });

  it('ignores profile checks the SDK cannot measure', () => {
    const resolved = resolveCheckSettings(DEFAULT_SELFIE_CHECKS, {
      FACE_OCCLUSION: { enabled: true, severity: 'error', max: 0.1 },
      FACE_PRESENT: { enabled: true, min: 1 },
    });
    expect(Object.keys(resolved)).toEqual(['FACE_PRESENT']);
  });

  it('applies overrides field by field', () => {
    const resolved = resolveCheckSettings(DEFAULT_SELFIE_CHECKS, undefined, {
      HEAD_POSE_YAW: { max: 10 },
      SHARPNESS: { severity: 'warning' },
      EYES_OPEN: { enabled: false },
    });
    expect(resolved.HEAD_POSE_YAW).toEqual({ enabled: true, severity: 'error', min: -15, max: 10 });
    expect(resolved.SHARPNESS).toEqual({ enabled: true, severity: 'warning', min: 0.3 });
    expect(resolved.EYES_OPEN?.enabled).toBe(false);
  });

  it('enables a check that an override adds to a profile without it', () => {
    const resolved = resolveCheckSettings(DEFAULT_SELFIE_CHECKS, {}, { INTER_EYE_DISTANCE: { min: 90 } });
    expect(resolved.INTER_EYE_DISTANCE).toEqual({ enabled: true, min: 90 });
  });

  it('does not mutate the defaults', () => {
    const before = JSON.stringify(DEFAULT_SELFIE_CHECKS);
    resolveCheckSettings(DEFAULT_SELFIE_CHECKS, undefined, { HEAD_POSE_YAW: { max: 1 } });
    expect(JSON.stringify(DEFAULT_SELFIE_CHECKS)).toBe(before);
  });

  it.each([
    ['min above max', { HEAD_POSE_YAW: { min: 10, max: -10 } }],
    ['non-finite bound', { SHARPNESS: { min: Number.NaN } }],
    ['infinite bound', { SHARPNESS: { max: Infinity } }],
    ['bad severity', { SHARPNESS: { severity: 'fatal' as unknown as 'error' } }],
    ['bad enabled', { SHARPNESS: { enabled: 'yes' as unknown as boolean } }],
  ])('rejects %s', (_name, overrides) => {
    expectInvalid(() => resolveCheckSettings(DEFAULT_SELFIE_CHECKS, undefined, overrides));
  });

  it('rejects unknown check IDs in overrides (typos must not silently disable guidance)', () => {
    expectInvalid(() =>
      resolveCheckSettings(DEFAULT_SELFIE_CHECKS, undefined, { HEAD_POSE_YAWW: { max: 1 } } as never),
    );
  });

  it('rejects malformed profile settings', () => {
    expectInvalid(() => resolveCheckSettings(DEFAULT_SELFIE_CHECKS, { SHARPNESS: { enabled: true, min: 2, max: 1 } }));
  });
});

describe('guidance configs', () => {
  it('selfie: defaults from the standard profile plus the SDK too-close bound', () => {
    const config = resolveSelfieGuidanceConfig({});
    expect(config.checks).toEqual(DEFAULT_SELFIE_CHECKS);
    expect(config.faceCountingRatio).toBe(0.02);
    expect(config.maxInterEyeRatio).toBe(0.35);
  });

  it('selfie: reads a full service profile (GET /v1/profiles/{name}) unchanged', () => {
    const config = resolveSelfieGuidanceConfig({
      profile: {
        name: 'icao-strict',
        version: '0.1.0',
        faceCounting: { minInterEyeDistanceRatio: 0.03 },
        selfie: { checks: { INTER_EYE_DISTANCE: { enabled: true, severity: 'error', min: 90 } } },
      },
      maxInterEyeRatio: null,
    });
    expect(config.checks).toEqual({ INTER_EYE_DISTANCE: { enabled: true, severity: 'error', min: 90 } });
    expect(config.faceCountingRatio).toBe(0.03);
    expect(config.maxInterEyeRatio).toBeNull();
  });

  it('document: defaults from the standard profile, no too-close bound', () => {
    const config = resolveDocumentGuidanceConfig({});
    expect(config.checks).toEqual(DEFAULT_DOCUMENT_CHECKS);
    expect(config.maxInterEyeRatio).toBeNull();
  });

  it('validates ratios and the face-counting ratio', () => {
    expect(validateRatio('r', 0.5)).toBe(0.5);
    expect(validateRatio('r', null)).toBeNull();
    expectInvalid(() => validateRatio('r', 0));
    expectInvalid(() => validateRatio('r', 1.5));
    expectInvalid(() => resolveFaceCountingRatio(undefined, -0.1));
    expect(resolveFaceCountingRatio()).toBe(0.02);
  });
});

describe('stability and session options', () => {
  it('merges stability overrides and rejects nonsense', () => {
    expect(resolveStability(DEFAULT_SELFIE_STABILITY, { windowMs: 900 }).windowMs).toBe(900);
    expectInvalid(() => resolveStability(DEFAULT_SELFIE_STABILITY, { windowMs: -1 }));
    expectInvalid(() => resolveStability(DEFAULT_SELFIE_STABILITY, { minFrames: 0 }));
  });

  const base = { video: {} as HTMLVideoElement } satisfies CommonCaptureOptions;

  it('fills in defaults', () => {
    const options = resolveSessionOptions(base, DEFAULT_SELFIE_STABILITY);
    expect(options).toMatchObject({
      autoCapture: true,
      burstCount: 0,
      jpegQuality: 0.92,
      maxAnalysisFps: 15,
      keepCameraOnCapture: false,
      mirrorPreview: null,
      delegate: 'CPU',
    });
    expect(options.stability).toEqual(DEFAULT_SELFIE_STABILITY);
  });

  it.each([
    { burstCount: 11 },
    { burstCount: 1.5 },
    { burstCount: -1 },
    { jpegQuality: 0 },
    { jpegQuality: 1.2 },
    { maxAnalysisFps: 0 },
    { maxAnalysisFps: 120 },
    { hintWindowMs: -5 },
    { initTimeoutMs: 0 },
    { delegate: 'NPU' as unknown as 'CPU' },
  ])('rejects %o', (bad) => {
    expectInvalid(() => resolveSessionOptions({ ...base, ...bad }, DEFAULT_SELFIE_STABILITY));
  });
});
