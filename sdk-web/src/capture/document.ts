import { resolveAssets } from '../analysis/assets';
import { FaceCaptureError } from '../errors';
import {
  DEFAULT_DOCUMENT_INTER_EYE_MARGIN,
  evaluateDocument,
  type DocumentGuidanceConfig,
  type DocumentMeasurements,
} from '../guidance/document';
import { DOCUMENT_GUIDANCE_CODES, type DocumentGuidanceCode } from '../guidance/types';
import { documentMessageKey } from '../i18n';
import { cardGuide, DEFAULT_GUIDE_FILL } from '../measure/card';
import { DEFAULT_DOCUMENT_CHECKS, DEFAULT_DOCUMENT_STABILITY, resolveCheckSettings, validateRatio } from '../thresholds';
import { pageUrl } from './page';
import { CaptureSession, resolveSessionOptions, type ModeStrategy } from './session';
import type { DocumentCaptureController, DocumentCaptureOptions } from './types';

/** Resolves the document guidance configuration (thresholds) from the options. Pure. */
export function resolveDocumentGuidanceConfig(
  options: Pick<DocumentCaptureOptions, 'profile' | 'checks' | 'maxInterEyeRatio' | 'interEyeDistanceMargin'>,
): DocumentGuidanceConfig {
  const margin = options.interEyeDistanceMargin ?? DEFAULT_DOCUMENT_INTER_EYE_MARGIN;
  if (!(Number.isFinite(margin) && margin >= 1 && margin <= 2)) {
    throw new FaceCaptureError('invalid-options', 'interEyeDistanceMargin must be within [1, 2]');
  }
  return {
    checks: resolveCheckSettings(DEFAULT_DOCUMENT_CHECKS, options.profile?.document?.checks, options.checks),
    maxInterEyeRatio: validateRatio('maxInterEyeRatio', options.maxInterEyeRatio ?? null),
    interEyeMargin: margin,
  };
}

/**
 * Guided capture of an identity document (ID-1 card) with the rear camera. The face detector finds
 * the portrait inside the card guide; guidance asks to move closer until the portrait's inter-eye
 * distance reaches the profile minimum, and flags glare and blur. The result is the full camera
 * frame as JPEG; the service finds and rectifies the card itself.
 */
export function createDocumentCapture(options: DocumentCaptureOptions): DocumentCaptureController {
  const config = resolveDocumentGuidanceConfig(options);
  const fill = options.guideFill ?? DEFAULT_GUIDE_FILL;
  if (!(fill > 0 && fill <= 1)) throw new FaceCaptureError('invalid-options', 'guideFill must be within (0, 1]');
  const assets = resolveAssets(options.assets, pageUrl());
  const strategy: ModeStrategy<DocumentGuidanceCode, DocumentMeasurements> = {
    mode: 'document',
    codes: DOCUMENT_GUIDANCE_CODES,
    holdStillCode: 'HOLD_STILL',
    readyCode: 'READY',
    modelUrl: assets.faceDetectorModelUrl,
    modelSha256: assets.faceDetectorModelSha256,
    defaultStability: DEFAULT_DOCUMENT_STABILITY,
    profile: options.profile,
    evaluate(analysis) {
      if (analysis.mode !== 'document') throw new FaceCaptureError('internal', 'Expected a document analysis');
      const m = analysis.measurements;
      const portrait = m.portrait;
      return {
        guidance: evaluateDocument(m, config),
        measurements: m,
        position: portrait?.eyeMidpoint ?? null,
        scale: portrait?.interEyeDistance ?? null,
        angles: [],
        sharpness: portrait?.sharpnessRaw ?? null,
      };
    },
    messageKey: documentMessageKey,
    guide: (width, height) => cardGuide(width, height, fill),
  };
  return new CaptureSession(strategy, options, resolveSessionOptions(options, DEFAULT_DOCUMENT_STABILITY), assets);
}
