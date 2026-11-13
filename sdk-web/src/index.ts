/**
 * @veriface/capture: browser capture SDK for Veriface.
 *
 * Guides a person to take a good selfie and a good photo of their ID document, auto-captures,
 * and hands the images to the host app, which uploads them to its own backend. The SDK never calls
 * the face service and makes no network requests other than loading its self-hosted assets.
 *
 * This entry is framework-agnostic and safe to import during server-side rendering. The custom
 * element lives in `@veriface/capture/element`.
 */

export { createSelfieCapture, resolveSelfieGuidanceConfig } from './capture/selfie';
export { createDocumentCapture, resolveDocumentGuidanceConfig } from './capture/document';
export type {
  BurstFrame,
  CaptureController,
  CaptureEventMap,
  CaptureMode,
  CaptureResult,
  CaptureStatus,
  CommonCaptureOptions,
  DocumentCaptureController,
  DocumentCaptureOptions,
  DocumentCaptureResult,
  DocumentGuidanceState,
  GuidanceState,
  GuidanceSummary,
  SelfieCaptureController,
  SelfieCaptureOptions,
  SelfieCaptureResult,
  SelfieGuidanceState,
} from './capture/types';
export type {
  AnalysisSignals,
  CameraSignals,
  CaptureSignals,
  FrameTimingSignals,
  StatsSnapshot,
  TrackSettingsSnapshot,
} from './capture/signals';
export { SELFIE_VIDEO_CONSTRAINTS, DOCUMENT_VIDEO_CONSTRAINTS } from './capture/camera';

export {
  ASSET_PATHS,
  DEFAULT_ASSET_BASE_URL,
  MODEL_SHA256,
  resolveAssets,
  type AssetOptions,
  type ResolvedAssets,
} from './analysis/assets';
export type { Delegate } from './analysis/types';
export type { EngineKind } from './analysis/engine';

export {
  CATALOGUE_VERSION,
  CHECK_IDS,
  CHECK_PRIORITY,
  DOCUMENT_SUBJECTS,
  SELFIE_SUBJECTS,
  type CheckId,
  type CheckStatus,
  type Severity,
  type Subject,
} from './contract';
export {
  DEFAULT_DOCUMENT_CHECKS,
  DEFAULT_DOCUMENT_STABILITY,
  DEFAULT_FACE_COUNTING_RATIO,
  DEFAULT_SELFIE_CHECKS,
  DEFAULT_SELFIE_MAX_INTER_EYE_RATIO,
  DEFAULT_SELFIE_STABILITY,
  resolveCheckSettings,
  type CheckOverrides,
  type CheckSetting,
  type CheckSettings,
  type ProfileLike,
  type StabilityOptions,
} from './thresholds';

export {
  DOCUMENT_GUIDANCE_CODES,
  SELFIE_GUIDANCE_CODES,
  type CheckEvaluation,
  type DocumentGuidanceCode,
  type FrameGuidance,
  type GuidanceCode,
  type GuidanceDirection,
  type GuidanceIssue,
  type SelfieGuidanceCode,
} from './guidance/types';
export { evaluateChecks, type CheckValues } from './guidance/evaluate';
export {
  evaluateSelfie,
  selfieCheckValues,
  type SelfieFace,
  type SelfieGuidanceConfig,
  type SelfieMeasurements,
} from './guidance/selfie';
export {
  documentCheckValues,
  evaluateDocument,
  type DocumentGuidanceConfig,
  type DocumentMeasurements,
  type DocumentPortrait,
} from './guidance/document';
export { StabilityTracker, type StabilitySample, type StabilityState } from './guidance/stability';
export type { Point, Rect } from './measure/geometry';
export { cardGuide, ID1_ASPECT } from './measure/card';

export {
  documentMessageKey,
  MESSAGES,
  resolveLocale,
  selfieMessageKey,
  textDirection,
  translate,
  type Locale,
  type MessageKey,
  type Messages,
  type UiMessageKey,
} from './i18n';

export { FaceCaptureError, isFaceCaptureError, type FaceCaptureErrorCode } from './errors';
export { SDK_VERSION } from './version';
