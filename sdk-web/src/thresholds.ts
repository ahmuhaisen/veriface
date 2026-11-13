import { CHECK_IDS, isCheckId, type CheckId, type Severity } from './contract';
import { FaceCaptureError } from './errors';

/**
 * One check's setting, in the same shape as `contracts/profile.schema.json` (`checkSetting`), so
 * a profile fetched from the service (`GET /v1/profiles/{name}`, through the project backend) can
 * be passed in unchanged. Bounds are inclusive; a missing bound is unbounded.
 */
export interface CheckSetting {
  readonly enabled: boolean;
  readonly severity?: Severity | undefined;
  readonly min?: number | undefined;
  readonly max?: number | undefined;
}

/** Check settings keyed by check ID. Unknown IDs (checks the SDK cannot measure) are ignored. */
export type CheckSettings = Readonly<Partial<Record<CheckId, CheckSetting>>>;

/** Per-check overrides; fields given here replace the base setting's fields. */
export type CheckOverrides = Readonly<Partial<Record<CheckId, Partial<CheckSetting>>>>;

/** The parts of a service profile (`contracts/profile.schema.json`) the SDK reads. */
export interface ProfileLike {
  readonly name?: string | undefined;
  readonly version?: string | undefined;
  readonly faceCounting?: { readonly minInterEyeDistanceRatio: number } | undefined;
  readonly selfie?: { readonly checks: Readonly<Record<string, CheckSetting>> } | undefined;
  readonly document?: { readonly checks: Readonly<Record<string, CheckSetting>> } | undefined;
}

/** Selfie check defaults: the `selfie` section of `profiles/standard.yaml` (v0.1.0). */
export const DEFAULT_SELFIE_CHECKS: CheckSettings = {
  IMAGE_RESOLUTION: { enabled: true, severity: 'error', min: 480 },
  FACE_PRESENT: { enabled: true, severity: 'error', min: 1 },
  SINGLE_FACE: { enabled: true, severity: 'error', max: 1 },
  INTER_EYE_DISTANCE: { enabled: true, severity: 'error', min: 60 },
  HEAD_IN_FRAME: { enabled: true, severity: 'error', min: 0 },
  HEAD_POSE_YAW: { enabled: true, severity: 'error', min: -15, max: 15 },
  HEAD_POSE_PITCH: { enabled: true, severity: 'error', min: -15, max: 15 },
  HEAD_POSE_ROLL: { enabled: true, severity: 'error', min: -10, max: 10 },
  EYES_OPEN: { enabled: true, severity: 'error', min: 0.4 },
  LUMINANCE_MEAN: { enabled: true, severity: 'error', min: 0.08, max: 0.75 },
  UNDER_EXPOSURE: { enabled: true, severity: 'error', max: 0.65 },
  OVER_EXPOSURE: { enabled: true, severity: 'error', max: 0.1 },
  SHARPNESS: { enabled: true, severity: 'error', min: 0.3 },
};

/** Document check defaults: the `document` section of `profiles/standard.yaml` (v0.1.0). */
export const DEFAULT_DOCUMENT_CHECKS: CheckSettings = {
  IMAGE_RESOLUTION: { enabled: true, severity: 'error', min: 640 },
  PORTRAIT_FOUND: { enabled: true, severity: 'error', min: 1 },
  INTER_EYE_DISTANCE: { enabled: true, severity: 'error', min: 60 },
  HEAD_POSE_YAW: { enabled: false },
  HEAD_POSE_PITCH: { enabled: false },
  HEAD_POSE_ROLL: { enabled: false },
  EYES_OPEN: { enabled: false },
  LUMINANCE_MEAN: { enabled: true, severity: 'warning', min: 0.06, max: 0.85 },
  UNDER_EXPOSURE: { enabled: true, severity: 'warning', max: 0.7 },
  OVER_EXPOSURE: { enabled: true, severity: 'error', max: 0.15 },
  SHARPNESS: { enabled: true, severity: 'error', min: 0.25 },
};

/** `faceCounting.minInterEyeDistanceRatio` of `profiles/standard.yaml` (NIST convention). */
export const DEFAULT_FACE_COUNTING_RATIO = 0.02;

/**
 * Resolves the effective settings for one subject section.
 *
 * - Without a profile section, the built-in defaults apply.
 * - With a profile section, it replaces the defaults entirely, with the profile's semantics:
 *   a check the section does not list is skipped.
 * - Overrides are applied last, field by field.
 *
 * @throws FaceCaptureError `invalid-options` when a setting is malformed.
 */
export function resolveCheckSettings(
  defaults: CheckSettings,
  profileChecks?: Readonly<Record<string, CheckSetting>>,
  overrides?: CheckOverrides,
): CheckSettings {
  const resolved: Partial<Record<CheckId, CheckSetting>> = {};
  const base: Readonly<Record<string, CheckSetting | undefined>> = profileChecks ?? defaults;
  for (const id of CHECK_IDS) {
    const setting = base[id];
    if (setting !== undefined) resolved[id] = { ...setting };
  }
  if (overrides) {
    // Overrides often come from JSON configuration, so they are checked as untyped input.
    for (const [key, override] of Object.entries(overrides as Readonly<Record<string, unknown>>)) {
      if (!isCheckId(key)) throw new FaceCaptureError('invalid-options', `Unknown check id in overrides: ${key}`);
      if (override === undefined) continue;
      if (typeof override !== 'object' || override === null) {
        throw new FaceCaptureError('invalid-options', `Override for ${key} must be an object`);
      }
      resolved[key] = { enabled: true, ...resolved[key], ...(override as Partial<CheckSetting>) };
    }
  }
  for (const id of CHECK_IDS) {
    const setting = resolved[id];
    if (setting !== undefined) validateCheckSetting(id, setting);
  }
  return resolved;
}

/** Validates a setting that may come from untyped input (JSON, a fetched profile). */
export function validateCheckSetting(id: string, setting: CheckSetting): void {
  const fail = (reason: string): never => {
    throw new FaceCaptureError('invalid-options', `Invalid setting for ${id}: ${reason}`);
  };
  const { enabled, severity, min, max } = setting as Readonly<Record<keyof CheckSetting, unknown>>;
  if (typeof enabled !== 'boolean') fail('`enabled` must be a boolean');
  if (severity !== undefined && severity !== 'error' && severity !== 'warning') {
    fail('`severity` must be "error" or "warning"');
  }
  if (min !== undefined && (typeof min !== 'number' || !Number.isFinite(min))) fail('`min` must be a finite number');
  if (max !== undefined && (typeof max !== 'number' || !Number.isFinite(max))) fail('`max` must be a finite number');
  if (typeof min === 'number' && typeof max === 'number' && min > max) {
    fail('`min` must not be greater than `max`');
  }
}

/** Reads `faceCounting.minInterEyeDistanceRatio` from a profile, with validation. */
export function resolveFaceCountingRatio(profile?: ProfileLike, override?: number): number {
  const ratio = override ?? profile?.faceCounting?.minInterEyeDistanceRatio ?? DEFAULT_FACE_COUNTING_RATIO;
  if (!Number.isFinite(ratio) || ratio < 0 || ratio > 0.5) {
    throw new FaceCaptureError('invalid-options', 'faceCounting ratio must be within [0, 0.5]');
  }
  return ratio;
}

/**
 * Capture-guidance settings that are NOT part of the service profile. They only shape the live
 * guidance; the service re-checks every image against its own profile.
 */
export interface StabilityOptions {
  /** How long the subject must stay still while passing every blocking check (ms). */
  readonly windowMs: number;
  /** Minimum number of analysed frames inside that window. */
  readonly minFrames: number;
  /** Maximum movement of the face (position or size), in inter-eye distances per second. */
  readonly maxMovementPerSecond: number;
  /** Maximum change of yaw, pitch or roll, in degrees per second (selfie only). */
  readonly maxRotationPerSecond: number;
  /**
   * Frames further apart than this break a still run (for example after the tab was hidden). It
   * must exceed the frame interval of slow devices, which may analyse only about 1 frame a second.
   */
  readonly maxFrameGapMs: number;
}

export const DEFAULT_SELFIE_STABILITY: StabilityOptions = {
  windowMs: 600,
  minFrames: 3,
  maxMovementPerSecond: 0.5,
  maxRotationPerSecond: 20,
  maxFrameGapMs: 1500,
};

export const DEFAULT_DOCUMENT_STABILITY: StabilityOptions = {
  windowMs: 800,
  minFrames: 4,
  maxMovementPerSecond: 0.5,
  maxRotationPerSecond: 20,
  maxFrameGapMs: 1500,
};

export function resolveStability(
  defaults: StabilityOptions,
  overrides?: Partial<StabilityOptions>,
): StabilityOptions {
  const resolved = { ...defaults, ...overrides };
  for (const [key, value] of Object.entries(resolved)) {
    if (!Number.isFinite(value) || value < 0) {
      throw new FaceCaptureError('invalid-options', `stability.${key} must be a non-negative number`);
    }
  }
  if (resolved.minFrames < 1) throw new FaceCaptureError('invalid-options', 'stability.minFrames must be at least 1');
  return resolved;
}

/**
 * "Too close" guidance bound: inter-eye distance as a fraction of the shorter frame side. SDK-only
 * (the profile has no upper bound): very close faces are distorted by the lens and are hard to keep
 * fully in frame. `null` disables it.
 */
export const DEFAULT_SELFIE_MAX_INTER_EYE_RATIO = 0.35;

export function validateRatio(name: string, value: number | null): number | null {
  if (value === null) return null;
  if (!Number.isFinite(value) || value <= 0 || value > 1) {
    throw new FaceCaptureError('invalid-options', `${name} must be within (0, 1] or null`);
  }
  return value;
}
