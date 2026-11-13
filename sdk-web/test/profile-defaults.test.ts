import { describe, expect, it } from 'vitest';
import { CHECK_IDS, CHECKS } from '../src/contract';
import {
  DEFAULT_DOCUMENT_CHECKS,
  DEFAULT_FACE_COUNTING_RATIO,
  DEFAULT_SELFIE_CHECKS,
  type CheckSetting,
  type CheckSettings,
} from '../src/thresholds';
import { readRepoYaml } from './helpers';

interface Profile {
  name: string;
  version: string;
  faceCounting: { minInterEyeDistanceRatio: number };
  selfie: { checks: Record<string, CheckSetting> };
  document: { checks: Record<string, CheckSetting> };
}

const standard = readRepoYaml('profiles/standard.yaml') as Profile;

/** The profile's settings for the checks the SDK measures and that apply to the section's subjects. */
function sdkSubset(checks: Record<string, CheckSetting>, subjects: string[]): CheckSettings {
  const subset: Record<string, CheckSetting> = {};
  for (const id of CHECK_IDS) {
    const setting = checks[id];
    if (setting !== undefined && CHECKS[id].subjects.some((s) => subjects.includes(s))) subset[id] = setting;
  }
  return subset;
}

describe('built-in defaults mirror profiles/standard.yaml', () => {
  it('uses the profile version the defaults were copied from', () => {
    expect(standard.name).toBe('standard');
    expect(standard.version).toBe('0.1.0');
  });

  it('matches the selfie section', () => {
    expect(DEFAULT_SELFIE_CHECKS).toEqual(sdkSubset(standard.selfie.checks, ['selfie']));
  });

  it('matches the document section', () => {
    expect(DEFAULT_DOCUMENT_CHECKS).toEqual(sdkSubset(standard.document.checks, ['document', 'portrait']));
  });

  it('matches faceCounting', () => {
    expect(DEFAULT_FACE_COUNTING_RATIO).toBe(standard.faceCounting.minInterEyeDistanceRatio);
  });

  it('uses the document inter-eye minimum for "move closer" (60 px in standard)', () => {
    expect(DEFAULT_DOCUMENT_CHECKS.INTER_EYE_DISTANCE?.min).toBe(60);
  });
});
