import { SELFIE_SUBJECTS } from '../contract';
import { headLargerThanFrame, worstSide } from '../measure/face-landmarks';
import type { Point, Rect } from '../measure/geometry';
import type { CheckSettings } from '../thresholds';
import { belowMinimum, evaluateChecks, issueFor, toFrameGuidance, type CheckValues } from './evaluate';
import type { CheckEvaluation, FrameGuidance, GuidanceIssue, SelfieGuidanceCode } from './types';

/** Measurements of the primary face (the one with the largest inter-eye distance). */
export interface SelfieFace {
  /** Inter-eye (iris centre) distance in pixels of the full-resolution frame. */
  readonly interEyeDistance: number;
  readonly eyeMidpoint: Point;
  /** Estimated head box (crown to chin, ear to ear), or null when it could not be estimated. */
  readonly headBox: Rect | null;
  /** HEAD_IN_FRAME value: smallest border margin divided by head height. */
  readonly headInFrame: number | null;
  /** Degrees, checks.yaml sign conventions (image coordinates of the un-mirrored frame). */
  readonly yaw: number | null;
  readonly pitch: number | null;
  readonly roll: number | null;
  /** EYES_OPEN value: openness of the less-open eye, 0-1. */
  readonly eyesOpen: number | null;
  readonly luminanceMean: number | null;
  readonly underExposure: number | null;
  readonly overExposure: number | null;
  /** SHARPNESS estimate 0-1 (provisional mapping). */
  readonly sharpness: number | null;
  /** Unbounded sharpness used to rank frames. */
  readonly sharpnessRaw: number | null;
}

/** Everything the guidance needs from one analysed selfie frame. */
export interface SelfieMeasurements {
  readonly frameWidth: number;
  readonly frameHeight: number;
  /** Inter-eye distance of every detected face, in pixels. */
  readonly faceInterEyeDistances: readonly number[];
  readonly face: SelfieFace | null;
}

export interface SelfieGuidanceConfig {
  readonly checks: CheckSettings;
  /** profile.faceCounting.minInterEyeDistanceRatio: smaller faces are not counted by SINGLE_FACE. */
  readonly faceCountingRatio: number;
  /** SDK-only "too close" bound: inter-eye distance / shorter frame side. Null disables it. */
  readonly maxInterEyeRatio: number | null;
}

/** Check values for a selfie frame, in the catalogue's units. */
export function selfieCheckValues(m: SelfieMeasurements, faceCountingRatio: number): CheckValues {
  const f = m.face;
  const minCounted = faceCountingRatio * m.frameWidth;
  return {
    IMAGE_RESOLUTION: Math.min(m.frameWidth, m.frameHeight),
    FACE_PRESENT: m.faceInterEyeDistances.length,
    SINGLE_FACE: m.faceInterEyeDistances.filter((d) => d >= minCounted).length,
    INTER_EYE_DISTANCE: f?.interEyeDistance ?? null,
    HEAD_IN_FRAME: f?.headInFrame ?? null,
    HEAD_POSE_YAW: f?.yaw ?? null,
    HEAD_POSE_PITCH: f?.pitch ?? null,
    HEAD_POSE_ROLL: f?.roll ?? null,
    EYES_OPEN: f?.eyesOpen ?? null,
    LUMINANCE_MEAN: f?.luminanceMean ?? null,
    UNDER_EXPOSURE: f?.underExposure ?? null,
    OVER_EXPOSURE: f?.overExposure ?? null,
    SHARPNESS: f?.sharpness ?? null,
  };
}

/** Evaluates a selfie frame and turns failed checks into guidance, root causes first. */
export function evaluateSelfie(m: SelfieMeasurements, config: SelfieGuidanceConfig): FrameGuidance<SelfieGuidanceCode> {
  const checks = evaluateChecks(selfieCheckValues(m, config.faceCountingRatio), config.checks, SELFIE_SUBJECTS);
  const issues: GuidanceIssue<SelfieGuidanceCode>[] = [];
  for (const evaluation of checks) {
    if (evaluation.status === 'fail') {
      issues.push(selfieIssue(evaluation, m));
    } else if (evaluation.id === 'INTER_EYE_DISTANCE' && evaluation.status === 'pass') {
      const tooClose = tooCloseIssue(evaluation, m, config.maxInterEyeRatio);
      if (tooClose) issues.push(tooClose);
    }
  }
  return toFrameGuidance(checks, issues);
}

function selfieIssue(e: CheckEvaluation, m: SelfieMeasurements): GuidanceIssue<SelfieGuidanceCode> {
  switch (e.id) {
    case 'IMAGE_RESOLUTION':
      return issueFor(e, 'LOW_RESOLUTION');
    case 'FACE_PRESENT':
    case 'PORTRAIT_FOUND':
      return issueFor(e, 'NO_FACE');
    case 'SINGLE_FACE':
      return issueFor(e, 'MULTIPLE_FACES');
    case 'HEAD_IN_FRAME': {
      const box = m.face?.headBox ?? null;
      if (box === null) return issueFor(e, 'HEAD_OUT_OF_FRAME');
      if (headLargerThanFrame(box, m.frameWidth, m.frameHeight)) return issueFor(e, 'TOO_CLOSE');
      return issueFor(e, 'HEAD_OUT_OF_FRAME', { direction: worstSide(box, m.frameWidth, m.frameHeight) });
    }
    case 'INTER_EYE_DISTANCE':
      return issueFor(e, belowMinimum(e) ? 'TOO_FAR' : 'TOO_CLOSE');
    case 'HEAD_POSE_YAW':
      return issueFor(e, 'YAW_OUT_OF_RANGE', { direction: belowMinimum(e) ? 'left' : 'right' });
    case 'HEAD_POSE_PITCH':
      return issueFor(e, 'PITCH_OUT_OF_RANGE', { direction: belowMinimum(e) ? 'down' : 'up' });
    case 'HEAD_POSE_ROLL':
      return issueFor(e, 'ROLL_OUT_OF_RANGE', {
        direction: belowMinimum(e) ? 'counterclockwise' : 'clockwise',
      });
    case 'EYES_OPEN':
      return issueFor(e, 'EYES_CLOSED');
    case 'OVER_EXPOSURE':
      return issueFor(e, 'TOO_BRIGHT');
    case 'UNDER_EXPOSURE':
      return issueFor(e, 'TOO_DARK');
    case 'LUMINANCE_MEAN':
      return issueFor(e, belowMinimum(e) ? 'TOO_DARK' : 'TOO_BRIGHT');
    case 'SHARPNESS':
      return issueFor(e, 'BLURRY');
  }
}

/** SDK-only upper bound on face size, expressed in pixels so it shares INTER_EYE_DISTANCE's unit. */
function tooCloseIssue(
  e: CheckEvaluation,
  m: SelfieMeasurements,
  maxInterEyeRatio: number | null,
): GuidanceIssue<SelfieGuidanceCode> | null {
  if (maxInterEyeRatio === null || e.value === null) return null;
  const shortSide = Math.min(m.frameWidth, m.frameHeight);
  // Compare ratios, not products: 0.35 * 720 is 251.99999999999997 in floating point.
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
