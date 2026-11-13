import { describe, expect, it } from 'vitest';
import { resolveDocumentGuidanceConfig } from '../src/capture/document';
import { documentCheckValues, evaluateDocument } from '../src/guidance/document';
import { documentMeasurements, documentPortrait } from './helpers';

const config = resolveDocumentGuidanceConfig({});

describe('document guidance', () => {
  it('passes a sharp, large, glare-free portrait', () => {
    const guidance = evaluateDocument(documentMeasurements(), config);
    expect(guidance.passed).toBe(true);
    expect(guidance.issues).toEqual([]);
    expect(guidance.advisories).toEqual([]);
  });

  it('NO_PORTRAIT when no face is found on the card', () => {
    const guidance = evaluateDocument(documentMeasurements(null), config);
    expect(guidance.issues.map((i) => i.code)).toEqual(['NO_PORTRAIT']);
    expect(guidance.checks.find((c) => c.id === 'INTER_EYE_DISTANCE')?.status).toBe('undetermined');
  });

  it('asks to move closer below the profile inter-eye minimum (60 px)', () => {
    const far = evaluateDocument(documentMeasurements(documentPortrait({ interEyeDistance: 45 })), config);
    expect(far.issues[0]).toMatchObject({ code: 'TOO_FAR', checkId: 'INTER_EYE_DISTANCE', value: 45, min: 60 });
    expect(far.issues[0]?.sdkOnly).toBeUndefined();
  });

  it('keeps asking until minimum x 1.15 (69 px), as an SDK-only margin', () => {
    const near = evaluateDocument(documentMeasurements(documentPortrait({ interEyeDistance: 62 })), config);
    expect(near.checks.find((c) => c.id === 'INTER_EYE_DISTANCE')?.status).toBe('pass');
    expect(near.issues).toHaveLength(1);
    expect(near.issues[0]).toMatchObject({ code: 'TOO_FAR', sdkOnly: true, value: 62 });
    expect(near.issues[0]?.min).toBeCloseTo(69, 9);
    expect(evaluateDocument(documentMeasurements(documentPortrait({ interEyeDistance: 69 })), config).passed).toBe(true);
  });

  it('makes the margin configurable, and validates it', () => {
    const exact = resolveDocumentGuidanceConfig({ interEyeDistanceMargin: 1 });
    expect(evaluateDocument(documentMeasurements(documentPortrait({ interEyeDistance: 60 })), exact).passed).toBe(true);
    const wide = resolveDocumentGuidanceConfig({ interEyeDistanceMargin: 1.5 });
    expect(evaluateDocument(documentMeasurements(documentPortrait({ interEyeDistance: 80 })), wide).passed).toBe(false);
    expect(() => resolveDocumentGuidanceConfig({ interEyeDistanceMargin: 0.9 })).toThrow(/interEyeDistanceMargin/);
    expect(config.interEyeMargin).toBe(1.15);
  });

  it('GLARE from clipped bright pixels on the portrait (OVER_EXPOSURE, error)', () => {
    const issues = evaluateDocument(documentMeasurements(documentPortrait({ overExposure: 0.2 })), config).issues;
    expect(issues[0]).toMatchObject({ code: 'GLARE', checkId: 'OVER_EXPOSURE', severity: 'error', max: 0.15 });
  });

  it('BLURRY below the document sharpness minimum', () => {
    const issues = evaluateDocument(documentMeasurements(documentPortrait({ sharpness: 0.2 })), config).issues;
    expect(issues.map((i) => i.code)).toEqual(['BLURRY']);
  });

  it('reports darkness as an advisory: it is warning-severity in the standard profile', () => {
    const guidance = evaluateDocument(documentMeasurements(documentPortrait({ luminanceMean: 0.03 })), config);
    expect(guidance.passed).toBe(true);
    expect(guidance.advisories.map((a) => a.code)).toEqual(['TOO_DARK']);
  });

  it('accepts ghost portraits: only the largest face matters', () => {
    const m = documentMeasurements(documentPortrait(), { portraitInterEyeDistances: [80, 25] });
    expect(documentCheckValues(m).PORTRAIT_FOUND).toBe(2);
    expect(evaluateDocument(m, config).passed).toBe(true);
  });

  it('LOW_RESOLUTION below the document minimum (640 px shorter side)', () => {
    const m = documentMeasurements(documentPortrait(), { frameWidth: 640, frameHeight: 480 });
    expect(evaluateDocument(m, config).issues.map((i) => i.code)).toEqual(['LOW_RESOLUTION']);
  });

  it('optional TOO_CLOSE bound for the portrait', () => {
    const withBound = resolveDocumentGuidanceConfig({ maxInterEyeRatio: 0.1 });
    const issue = evaluateDocument(documentMeasurements(documentPortrait({ interEyeDistance: 120 })), withBound).issues[0];
    expect(issue).toMatchObject({ code: 'TOO_CLOSE', sdkOnly: true });
    expect(issue?.max).toBeCloseTo(108, 9);
    expect(evaluateDocument(documentMeasurements(documentPortrait({ interEyeDistance: 108 })), withBound).passed).toBe(true);
  });

  it('never evaluates selfie-only or landmark-based checks for documents', () => {
    const ids = evaluateDocument(documentMeasurements(), config).checks.map((c) => c.id);
    expect(ids).not.toContain('FACE_PRESENT');
    expect(ids).not.toContain('HEAD_IN_FRAME');
    const yaw = evaluateDocument(documentMeasurements(), config).checks.find((c) => c.id === 'HEAD_POSE_YAW');
    expect(yaw?.status).toBe('skipped');
  });
});
