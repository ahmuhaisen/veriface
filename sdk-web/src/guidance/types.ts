import type { CheckId, CheckStatus, Severity } from '../contract';

/**
 * Guidance codes for selfie capture, in catalogue priority order of the checks they come from.
 * Each code except HOLD_STILL and READY maps to a check ID.
 */
export const SELFIE_GUIDANCE_CODES = [
  'NO_FACE', // FACE_PRESENT
  'MULTIPLE_FACES', // SINGLE_FACE
  'LOW_RESOLUTION', // IMAGE_RESOLUTION
  'HEAD_OUT_OF_FRAME', // HEAD_IN_FRAME
  'TOO_CLOSE', // HEAD_IN_FRAME with a head larger than the frame, or INTER_EYE_DISTANCE above the SDK bound
  'TOO_FAR', // INTER_EYE_DISTANCE below the minimum
  'YAW_OUT_OF_RANGE', // HEAD_POSE_YAW
  'PITCH_OUT_OF_RANGE', // HEAD_POSE_PITCH
  'ROLL_OUT_OF_RANGE', // HEAD_POSE_ROLL
  'EYES_CLOSED', // EYES_OPEN
  'TOO_BRIGHT', // OVER_EXPOSURE, or LUMINANCE_MEAN above the maximum
  'TOO_DARK', // UNDER_EXPOSURE, or LUMINANCE_MEAN below the minimum
  'BLURRY', // SHARPNESS
  'HOLD_STILL', // every blocking check passes; waiting for a still moment
  'READY', // still long enough; capturing
] as const;

/** Guidance codes for document capture, in catalogue priority order of the checks they come from. */
export const DOCUMENT_GUIDANCE_CODES = [
  'NO_PORTRAIT', // PORTRAIT_FOUND
  'LOW_RESOLUTION', // IMAGE_RESOLUTION
  'TOO_CLOSE', // INTER_EYE_DISTANCE above the optional SDK bound
  'TOO_FAR', // INTER_EYE_DISTANCE of the portrait below the minimum
  'GLARE', // OVER_EXPOSURE on the portrait
  'TOO_DARK', // UNDER_EXPOSURE, or LUMINANCE_MEAN below the minimum
  'TOO_BRIGHT', // LUMINANCE_MEAN above the maximum
  'BLURRY', // SHARPNESS
  'HOLD_STILL',
  'READY',
] as const;

export type SelfieGuidanceCode = (typeof SELFIE_GUIDANCE_CODES)[number];
export type DocumentGuidanceCode = (typeof DOCUMENT_GUIDANCE_CODES)[number];
export type GuidanceCode = SelfieGuidanceCode | DocumentGuidanceCode;

/**
 * Where a problem points, in IMAGE coordinates of the un-mirrored capture (checks.yaml sign
 * conventions). The selfie preview is mirrored, so left and right are swapped on screen.
 * - yaw: the side the face points towards; pitch: up or down; roll: rotation in the image;
 * - head out of frame: the image side where the head is cut off.
 */
export type GuidanceDirection =
  | 'left'
  | 'right'
  | 'up'
  | 'down'
  | 'clockwise'
  | 'counterclockwise'
  | 'top'
  | 'bottom';

/** One failed check, translated into guidance. */
export interface GuidanceIssue<C extends GuidanceCode = GuidanceCode> {
  readonly code: C;
  readonly checkId: CheckId;
  readonly severity: Severity;
  /** The measured value, in the check's unit. */
  readonly value: number | null;
  /** The bounds that apply (profile bounds, or the SDK bound when `sdkOnly`). */
  readonly min: number | null;
  readonly max: number | null;
  readonly direction?: GuidanceDirection;
  /** True when the bound comes from SDK-only guidance, not from the profile. */
  readonly sdkOnly?: boolean;
}

/** One check evaluated on one frame, in the shape of the API's `CheckResult` (without `group`). */
export interface CheckEvaluation {
  readonly id: CheckId;
  readonly status: CheckStatus;
  readonly severity: Severity;
  readonly value: number | null;
  readonly unit: string;
  readonly min: number | null;
  readonly max: number | null;
}

/** Guidance for one analysed frame. */
export interface FrameGuidance<C extends GuidanceCode = GuidanceCode> {
  readonly checks: readonly CheckEvaluation[];
  /** Failed error-severity checks: they block auto-capture. Priority order (root causes first). */
  readonly issues: readonly GuidanceIssue<C>[];
  /** Failed warning-severity checks: reported, but they never block capture. */
  readonly advisories: readonly GuidanceIssue<C>[];
  /** True when no blocking issue remains. */
  readonly passed: boolean;
}
