import { resolveAssets } from '../analysis/assets';
import { FaceCaptureError } from '../errors';
import { evaluateSelfie, type SelfieGuidanceConfig, type SelfieMeasurements } from '../guidance/selfie';
import { SELFIE_GUIDANCE_CODES, type SelfieGuidanceCode } from '../guidance/types';
import { selfieMessageKey } from '../i18n';
import {
  DEFAULT_SELFIE_CHECKS,
  DEFAULT_SELFIE_MAX_INTER_EYE_RATIO,
  DEFAULT_SELFIE_STABILITY,
  resolveCheckSettings,
  resolveFaceCountingRatio,
  validateRatio,
} from '../thresholds';
import { pageUrl } from './page';
import { CaptureSession, resolveSessionOptions, type ModeStrategy } from './session';
import type { SelfieCaptureController, SelfieCaptureOptions } from './types';

/** Resolves the selfie guidance configuration (thresholds) from the options. Pure. */
export function resolveSelfieGuidanceConfig(
  options: Pick<SelfieCaptureOptions, 'profile' | 'checks' | 'maxInterEyeRatio'>,
): SelfieGuidanceConfig {
  return {
    checks: resolveCheckSettings(DEFAULT_SELFIE_CHECKS, options.profile?.selfie?.checks, options.checks),
    faceCountingRatio: resolveFaceCountingRatio(options.profile),
    maxInterEyeRatio: validateRatio(
      'maxInterEyeRatio',
      options.maxInterEyeRatio === undefined ? DEFAULT_SELFIE_MAX_INTER_EYE_RATIO : options.maxInterEyeRatio,
    ),
  };
}

/**
 * Guided selfie capture with the front camera: live guidance, a stability window, and automatic
 * capture of the sharpest still frame. The result is a full-resolution, un-mirrored JPEG for the
 * host app to upload to its own backend. Nothing is sent anywhere by the SDK.
 */
export function createSelfieCapture(options: SelfieCaptureOptions): SelfieCaptureController {
  const config = resolveSelfieGuidanceConfig(options);
  const assets = resolveAssets(options.assets, pageUrl());
  const strategy: ModeStrategy<SelfieGuidanceCode, SelfieMeasurements> = {
    mode: 'selfie',
    codes: SELFIE_GUIDANCE_CODES,
    holdStillCode: 'HOLD_STILL',
    readyCode: 'READY',
    modelUrl: assets.faceLandmarkerModelUrl,
    modelSha256: assets.faceLandmarkerModelSha256,
    defaultStability: DEFAULT_SELFIE_STABILITY,
    profile: options.profile,
    evaluate(analysis) {
      if (analysis.mode !== 'selfie') throw new FaceCaptureError('internal', 'Expected a selfie analysis');
      const m = analysis.measurements;
      const face = m.face;
      return {
        guidance: evaluateSelfie(m, config),
        measurements: m,
        position: face?.eyeMidpoint ?? null,
        scale: face?.interEyeDistance ?? null,
        angles: [face?.yaw ?? null, face?.pitch ?? null, face?.roll ?? null],
        sharpness: face?.sharpnessRaw ?? null,
      };
    },
    messageKey: selfieMessageKey,
    guide: () => null,
  };
  return new CaptureSession(strategy, options, resolveSessionOptions(options, DEFAULT_SELFIE_STABILITY), assets);
}
