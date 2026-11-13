/**
 * The part of the check catalogue (`contracts/checks.yaml`, catalogueVersion 1) that the browser
 * SDK measures. The catalogue is the source of truth; `test/contract.test.ts` fails when this file
 * drifts from it.
 */

export const CATALOGUE_VERSION = 1;

/** Check IDs the SDK can estimate in the browser, from the catalogue. */
export type CheckId =
  | 'IMAGE_RESOLUTION'
  | 'FACE_PRESENT'
  | 'SINGLE_FACE'
  | 'PORTRAIT_FOUND'
  | 'INTER_EYE_DISTANCE'
  | 'HEAD_IN_FRAME'
  | 'HEAD_POSE_YAW'
  | 'HEAD_POSE_PITCH'
  | 'HEAD_POSE_ROLL'
  | 'EYES_OPEN'
  | 'LUMINANCE_MEAN'
  | 'UNDER_EXPOSURE'
  | 'OVER_EXPOSURE'
  | 'SHARPNESS';

export type Subject = 'selfie' | 'document' | 'portrait';
export type Severity = 'error' | 'warning';
export type CheckStatus = 'pass' | 'fail' | 'undetermined' | 'skipped';

/** Subjects of an assessment: what a capture is checked as. */
export const SELFIE_SUBJECTS: readonly Subject[] = ['selfie'];
export const DOCUMENT_SUBJECTS: readonly Subject[] = ['document', 'portrait'];

interface CheckDefinition {
  readonly subjects: readonly Subject[];
  readonly unit: string;
  /** Catalogue prerequisites, restricted to checks the SDK measures. */
  readonly prerequisites: readonly CheckId[];
  readonly defaultSeverity: Severity;
}

export const CHECKS: Readonly<Record<CheckId, CheckDefinition>> = {
  IMAGE_RESOLUTION: { subjects: ['selfie', 'document'], unit: 'px', prerequisites: [], defaultSeverity: 'error' },
  FACE_PRESENT: { subjects: ['selfie'], unit: 'count', prerequisites: [], defaultSeverity: 'error' },
  SINGLE_FACE: { subjects: ['selfie'], unit: 'count', prerequisites: ['FACE_PRESENT'], defaultSeverity: 'error' },
  PORTRAIT_FOUND: { subjects: ['document'], unit: 'count', prerequisites: [], defaultSeverity: 'error' },
  INTER_EYE_DISTANCE: {
    subjects: ['selfie', 'portrait'],
    unit: 'px',
    prerequisites: ['FACE_PRESENT', 'PORTRAIT_FOUND'],
    defaultSeverity: 'error',
  },
  HEAD_IN_FRAME: {
    subjects: ['selfie'],
    unit: 'ratio',
    prerequisites: ['FACE_PRESENT', 'SINGLE_FACE'],
    defaultSeverity: 'error',
  },
  HEAD_POSE_YAW: {
    subjects: ['selfie', 'portrait'],
    unit: 'deg',
    prerequisites: ['FACE_PRESENT', 'SINGLE_FACE', 'PORTRAIT_FOUND'],
    defaultSeverity: 'error',
  },
  HEAD_POSE_PITCH: {
    subjects: ['selfie', 'portrait'],
    unit: 'deg',
    prerequisites: ['FACE_PRESENT', 'SINGLE_FACE', 'PORTRAIT_FOUND'],
    defaultSeverity: 'error',
  },
  HEAD_POSE_ROLL: {
    subjects: ['selfie', 'portrait'],
    unit: 'deg',
    prerequisites: ['FACE_PRESENT', 'SINGLE_FACE', 'PORTRAIT_FOUND'],
    defaultSeverity: 'error',
  },
  // The catalogue also lists EYES_VISIBLE as a prerequisite. The SDK does not measure it, so it
  // is treated as skipped, and a skipped prerequisite is ignored.
  EYES_OPEN: {
    subjects: ['selfie', 'portrait'],
    unit: 'score',
    prerequisites: ['FACE_PRESENT', 'SINGLE_FACE', 'PORTRAIT_FOUND'],
    defaultSeverity: 'error',
  },
  LUMINANCE_MEAN: {
    subjects: ['selfie', 'portrait'],
    unit: 'fraction',
    prerequisites: ['FACE_PRESENT', 'SINGLE_FACE', 'PORTRAIT_FOUND'],
    defaultSeverity: 'error',
  },
  UNDER_EXPOSURE: {
    subjects: ['selfie', 'portrait'],
    unit: 'fraction',
    prerequisites: ['FACE_PRESENT', 'SINGLE_FACE', 'PORTRAIT_FOUND'],
    defaultSeverity: 'error',
  },
  OVER_EXPOSURE: {
    subjects: ['selfie', 'portrait'],
    unit: 'fraction',
    prerequisites: ['FACE_PRESENT', 'SINGLE_FACE', 'PORTRAIT_FOUND'],
    defaultSeverity: 'error',
  },
  SHARPNESS: {
    subjects: ['selfie', 'portrait'],
    unit: 'score',
    prerequisites: ['FACE_PRESENT', 'SINGLE_FACE', 'PORTRAIT_FOUND'],
    defaultSeverity: 'error',
  },
};

export const CHECK_IDS = Object.keys(CHECKS) as readonly CheckId[];

/**
 * Catalogue `reasons.priority` (root causes first), restricted to the SDK's checks. Guidance shows
 * the first failing check in this order, and prerequisites always come before their dependants.
 */
export const CHECK_PRIORITY: readonly CheckId[] = [
  'FACE_PRESENT',
  'PORTRAIT_FOUND',
  'SINGLE_FACE',
  'IMAGE_RESOLUTION',
  'HEAD_IN_FRAME',
  'INTER_EYE_DISTANCE',
  'HEAD_POSE_YAW',
  'HEAD_POSE_PITCH',
  'HEAD_POSE_ROLL',
  'EYES_OPEN',
  'OVER_EXPOSURE',
  'UNDER_EXPOSURE',
  'LUMINANCE_MEAN',
  'SHARPNESS',
];

export function isCheckId(value: string): value is CheckId {
  return Object.hasOwn(CHECKS, value);
}

/** True when the check applies to at least one subject of the assessment. */
export function appliesTo(id: CheckId, subjects: readonly Subject[]): boolean {
  return CHECKS[id].subjects.some((subject) => subjects.includes(subject));
}
