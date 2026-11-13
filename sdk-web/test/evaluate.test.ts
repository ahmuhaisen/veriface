import { describe, expect, it } from 'vitest';
import { DOCUMENT_SUBJECTS, SELFIE_SUBJECTS, type CheckId } from '../src/contract';
import { evaluateChecks, withinBounds } from '../src/guidance/evaluate';
import type { CheckEvaluation } from '../src/guidance/types';
import { DEFAULT_SELFIE_CHECKS } from '../src/thresholds';

function status(results: CheckEvaluation[], id: CheckId) {
  return results.find((r) => r.id === id)?.status;
}

describe('withinBounds', () => {
  it('is inclusive and treats a missing bound as unbounded', () => {
    expect(withinBounds(15, -15, 15)).toBe(true);
    expect(withinBounds(-15, -15, 15)).toBe(true);
    expect(withinBounds(15.0001, -15, 15)).toBe(false);
    expect(withinBounds(1e9, 60, undefined)).toBe(true);
    expect(withinBounds(-1e9, undefined, 1)).toBe(true);
    expect(withinBounds(0, undefined, undefined)).toBe(true);
  });
});

describe('evaluateChecks (catalogue status rules)', () => {
  it('passes and fails on the profile bounds', () => {
    const results = evaluateChecks(
      { FACE_PRESENT: 1, SINGLE_FACE: 1, HEAD_POSE_YAW: 15, HEAD_POSE_PITCH: -16 },
      DEFAULT_SELFIE_CHECKS,
      SELFIE_SUBJECTS,
    );
    expect(status(results, 'HEAD_POSE_YAW')).toBe('pass');
    expect(status(results, 'HEAD_POSE_PITCH')).toBe('fail');
  });

  it('marks checks without a value as undetermined', () => {
    const results = evaluateChecks(
      { FACE_PRESENT: 1, SINGLE_FACE: 1, HEAD_POSE_YAW: null, SHARPNESS: Number.NaN },
      DEFAULT_SELFIE_CHECKS,
      SELFIE_SUBJECTS,
    );
    expect(status(results, 'HEAD_POSE_YAW')).toBe('undetermined');
    expect(status(results, 'SHARPNESS')).toBe('undetermined');
    expect(results.find((r) => r.id === 'SHARPNESS')?.value).toBeNull();
  });

  it('marks dependants undetermined when a prerequisite fails', () => {
    const results = evaluateChecks(
      { FACE_PRESENT: 0, SINGLE_FACE: 0, HEAD_IN_FRAME: 0.2, HEAD_POSE_YAW: 0 },
      DEFAULT_SELFIE_CHECKS,
      SELFIE_SUBJECTS,
    );
    expect(status(results, 'FACE_PRESENT')).toBe('fail');
    expect(status(results, 'SINGLE_FACE')).toBe('undetermined');
    expect(status(results, 'HEAD_IN_FRAME')).toBe('undetermined');
    expect(status(results, 'HEAD_POSE_YAW')).toBe('undetermined');
  });

  it('ignores prerequisites whose subject is not part of the assessment', () => {
    // INTER_EYE_DISTANCE lists PORTRAIT_FOUND (document subject); a selfie must not wait for it.
    const results = evaluateChecks({ FACE_PRESENT: 1, INTER_EYE_DISTANCE: 100 }, DEFAULT_SELFIE_CHECKS, SELFIE_SUBJECTS);
    expect(results.some((r) => r.id === 'PORTRAIT_FOUND')).toBe(false);
    expect(status(results, 'INTER_EYE_DISTANCE')).toBe('pass');
  });

  it('ignores skipped prerequisites', () => {
    const settings = { ...DEFAULT_SELFIE_CHECKS, SINGLE_FACE: { enabled: false } };
    const results = evaluateChecks({ FACE_PRESENT: 1, SINGLE_FACE: 5, HEAD_IN_FRAME: 0.1 }, settings, SELFIE_SUBJECTS);
    expect(status(results, 'SINGLE_FACE')).toBe('skipped');
    expect(status(results, 'HEAD_IN_FRAME')).toBe('pass');
  });

  it('skips disabled and unlisted checks', () => {
    const results = evaluateChecks({ FACE_PRESENT: 1, EYES_OPEN: 0 }, { FACE_PRESENT: { enabled: true, min: 1 } }, SELFIE_SUBJECTS);
    expect(status(results, 'EYES_OPEN')).toBe('skipped');
    expect(status(results, 'FACE_PRESENT')).toBe('pass');
  });

  it('uses the catalogue default severity when the setting has none', () => {
    const results = evaluateChecks({ FACE_PRESENT: 1 }, { FACE_PRESENT: { enabled: true, min: 1 } }, SELFIE_SUBJECTS);
    expect(results.find((r) => r.id === 'FACE_PRESENT')?.severity).toBe('error');
  });

  it('reports units and bounds like the API CheckResult', () => {
    const results = evaluateChecks({ FACE_PRESENT: 1, SINGLE_FACE: 1, HEAD_POSE_ROLL: 3 }, DEFAULT_SELFIE_CHECKS, SELFIE_SUBJECTS);
    expect(results.find((r) => r.id === 'HEAD_POSE_ROLL')).toEqual({
      id: 'HEAD_POSE_ROLL',
      status: 'pass',
      severity: 'error',
      value: 3,
      unit: 'deg',
      min: -10,
      max: 10,
    });
  });

  it('evaluates only checks that apply to the assessment subjects, in priority order', () => {
    const selfie = evaluateChecks({}, DEFAULT_SELFIE_CHECKS, SELFIE_SUBJECTS).map((r) => r.id);
    expect(selfie).not.toContain('PORTRAIT_FOUND');
    expect(selfie[0]).toBe('FACE_PRESENT');
    const document = evaluateChecks({}, {}, DOCUMENT_SUBJECTS).map((r) => r.id);
    expect(document).not.toContain('FACE_PRESENT');
    expect(document).not.toContain('HEAD_IN_FRAME');
    expect(document[0]).toBe('PORTRAIT_FOUND');
  });
});
