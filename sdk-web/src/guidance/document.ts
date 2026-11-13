import { DOCUMENT_SUBJECTS } from '../contract';
import type { Point, Rect } from '../measure/geometry';
import type { CheckSettings } from '../thresholds';
import { belowMinimum, evaluateChecks, issueFor, toFrameGuidance, type CheckValues } from './evaluate';
import type { CheckEvaluation, DocumentGuidanceCode, FrameGuidance, GuidanceIssue } from './types';

/** Measurements of the selected document portrait (the largest face on the card). */
export interface DocumentPortrait {
  /** Inter-eye distance in pixels of the full-resolution frame (from the face detector's eye keypoints). */
  readonly interEyeDistance: number;
  readonly eyeMidpoint: Point;
  /** Face box of the portrait in frame pixels. */
  readonly box: Rect;
  readonly luminanceMean: number | null;
  readonly underExposure: number | null;
  /** Fraction of clipped bright pixels over the portrait's face: glare. */
  readonly overExposure: number | null;
  readonly sharpness: number | null;
  readonly sharpnessRaw: number | null;
}

/** Everything the guidance needs from one analysed document frame. */
export interface DocumentMeasurements {
  readonly frameWidth: number;
  readonly frameHeight: number;
  /** The card guide (region searched for the portrait), in frame pixels. */
  readonly guide: Rect;
  /** Inter-eye distance of every portrait candidate, in pixels. */
  readonly portraitInterEyeDistances: readonly number[];
  readonly portrait: DocumentPortrait | null;
}

export interface DocumentGuidanceConfig {
  readonly checks: CheckSettings;
  /** Optional SDK-only "too close" bound: portrait inter-eye distance / shorter frame side. */
  readonly maxInterEyeRatio: number | null;
  /** Guide to at least INTER_EYE_DISTANCE.min x this factor (>= 1). Default 1.15. */
  readonly interEyeMargin: number;
}

/** Default document inter-eye safety margin (detector vs iris-based distance, about ±8%). */
export const DEFAULT_DOCUMENT_INTER_EYE_MARGIN = 1.15;

/** Check values for a document frame, in the catalogue's units. */
export function documentCheckValues(m: DocumentMeasurements): CheckValues {
  const p = m.portrait;
  return {
    IMAGE_RESOLUTION: Math.min(m.frameWidth, m.frameHeight),
    PORTRAIT_FOUND: m.portraitInterEyeDistances.length,
    INTER_EYE_DISTANCE: p?.interEyeDistance ?? null,
    LUMINANCE_MEAN: p?.luminanceMean ?? null,
    UNDER_EXPOSURE: p?.underExposure ?? null,
    OVER_EXPOSURE: p?.overExposure ?? null,
    SHARPNESS: p?.sharpness ?? null,
  };
}

/** Evaluates a document frame and turns failed checks into guidance, root causes first. */
export function evaluateDocument(
  m: DocumentMeasurements,
  config: DocumentGuidanceConfig,
): FrameGuidance<DocumentGuidanceCode> {
  const checks = evaluateChecks(documentCheckValues(m), config.checks, DOCUMENT_SUBJECTS);
  const issues: GuidanceIssue<DocumentGuidanceCode>[] = [];
  for (const evaluation of checks) {
    if (evaluation.status === 'fail') {
      const issue = documentIssue(evaluation);
      if (issue) issues.push(issue);
    } else if (evaluation.id === 'INTER_EYE_DISTANCE' && evaluation.status === 'pass') {
      const tooFar = marginIssue(evaluation, config.interEyeMargin);
      const tooClose = tooFar ? null : tooCloseIssue(evaluation, m, config.maxInterEyeRatio);
      if (tooFar) issues.push(tooFar);
      if (tooClose) issues.push(tooClose);
    }
  }
  return toFrameGuidance(checks, issues);
}

/**
 * SDK-only safety margin: the portrait passes the profile minimum but not minimum x margin. The
 * detector's eye keypoints differ from the service's iris-based distance by up to about 8%, so
 * guidance asks for a little more than the minimum.
 */
function marginIssue(e: CheckEvaluation, margin: number): GuidanceIssue<DocumentGuidanceCode> | null {
  if (e.value === null || e.min === null || margin <= 1) return null;
  const target = e.min * margin;
  if (e.value >= target) return null;
  return {
    code: 'TOO_FAR',
    checkId: 'INTER_EYE_DISTANCE',
    severity: e.severity,
    value: e.value,
    min: target,
    max: null,
    sdkOnly: true,
  };
}

function documentIssue(e: CheckEvaluation): GuidanceIssue<DocumentGuidanceCode> | null {
  switch (e.id) {
    case 'IMAGE_RESOLUTION':
      return issueFor(e, 'LOW_RESOLUTION');
    case 'PORTRAIT_FOUND':
      return issueFor(e, 'NO_PORTRAIT');
    case 'INTER_EYE_DISTANCE':
      return issueFor(e, belowMinimum(e) ? 'TOO_FAR' : 'TOO_CLOSE');
    case 'OVER_EXPOSURE':
      return issueFor(e, 'GLARE');
    case 'UNDER_EXPOSURE':
      return issueFor(e, 'TOO_DARK');
    case 'LUMINANCE_MEAN':
      return issueFor(e, belowMinimum(e) ? 'TOO_DARK' : 'TOO_BRIGHT');
    case 'SHARPNESS':
      return issueFor(e, 'BLURRY');
    default:
      // Pose and eye checks need face landmarks; the document flow uses the face detector only.
      return null;
  }
}

function tooCloseIssue(
  e: CheckEvaluation,
  m: DocumentMeasurements,
  maxInterEyeRatio: number | null,
): GuidanceIssue<DocumentGuidanceCode> | null {
  if (maxInterEyeRatio === null || e.value === null) return null;
  const shortSide = Math.min(m.frameWidth, m.frameHeight);
  if (e.value / shortSide <= maxInterEyeRatio) return null;
  return {
    code: 'TOO_CLOSE',
    checkId: 'INTER_EYE_DISTANCE',
    severity: e.severity,
    value: e.value,
    min: null,
    max: maxInterEyeRatio * shortSide,
    sdkOnly: true,
  };
}
