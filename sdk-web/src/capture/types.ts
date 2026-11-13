import type { AssetOptions } from '../analysis/assets';
import type { EngineKind } from '../analysis/engine';
import type { CaptureMode, Delegate } from '../analysis/types';
import type { FaceCaptureError } from '../errors';
import type { DocumentMeasurements } from '../guidance/document';
import type { SelfieMeasurements } from '../guidance/selfie';
import type { StabilityState } from '../guidance/stability';
import type {
  CheckEvaluation,
  DocumentGuidanceCode,
  GuidanceCode,
  GuidanceIssue,
  SelfieGuidanceCode,
} from '../guidance/types';
import type { MessageKey } from '../i18n';
import type { Rect } from '../measure/geometry';
import type { CheckOverrides, ProfileLike, StabilityOptions } from '../thresholds';
import type { CaptureSignals } from './signals';

export type { CaptureMode } from '../analysis/types';

export type CaptureStatus =
  | 'idle'
  | 'starting'
  | 'running'
  | 'capturing'
  | 'captured'
  | 'stopped'
  | 'error'
  | 'disposed';

/** Live guidance, emitted for every analysed frame. */
export interface GuidanceState<C extends GuidanceCode, M> {
  readonly mode: CaptureMode;
  /** The hint to show now, smoothed over recent frames so it does not flicker. */
  readonly code: C;
  /** Message key of `code` (see the i18n module). */
  readonly messageKey: MessageKey;
  /** This frame's own code, before smoothing. */
  readonly frameCode: C;
  /** Blocking issues of this frame, root causes first. */
  readonly issues: readonly GuidanceIssue<C>[];
  /** Non-blocking (warning-severity) issues of this frame. */
  readonly advisories: readonly GuidanceIssue<C>[];
  /** Every check of this frame, as the SDK estimated it. */
  readonly checks: readonly CheckEvaluation[];
  /** No blocking issue on this frame. */
  readonly passed: boolean;
  readonly stability: StabilityState;
  readonly measurements: M;
  readonly frame: {
    readonly width: number;
    readonly height: number;
    readonly timestamp: number;
    readonly analysisMs: number;
  };
}

/** How the captured frame looked to the SDK. The service re-checks the image regardless. */
export interface GuidanceSummary<C extends GuidanceCode, M> {
  readonly trigger: 'auto' | 'manual';
  /** The capture was taken from a still run in which every blocking check passed. */
  readonly ready: boolean;
  readonly issues: readonly GuidanceIssue<C>[];
  readonly advisories: readonly GuidanceIssue<C>[];
  readonly checks: readonly CheckEvaluation[];
  readonly measurements: M;
  /** Length of the still run at capture time, ms. */
  readonly stableMs: number;
  /** Where the thresholds came from. */
  readonly profile: { readonly name: string | null; readonly version: string | null };
  readonly catalogueVersion: number;
}

export interface BurstFrame {
  readonly blob: Blob;
  readonly width: number;
  readonly height: number;
  /** Time relative to the main captured frame, ms (negative = earlier). */
  readonly offsetMs: number;
}

export interface CaptureResult<C extends GuidanceCode, M> {
  readonly mode: CaptureMode;
  /** JPEG at the camera's full resolution. Never mirrored. */
  readonly blob: Blob;
  readonly mimeType: 'image/jpeg';
  readonly width: number;
  readonly height: number;
  /** ISO 8601 UTC time of the capture. */
  readonly capturedAt: string;
  readonly guidance: GuidanceSummary<C, M>;
  /** Deterrence and diagnostics only; NOT a security signal. */
  readonly signals: CaptureSignals;
  /** The last frames before the capture, in time order (empty unless `burstCount` > 0). */
  readonly burst: readonly BurstFrame[];
}

export interface CaptureEventMap<C extends GuidanceCode, M> {
  guidance: GuidanceState<C, M>;
  capture: CaptureResult<C, M>;
  error: FaceCaptureError;
  status: CaptureStatus;
}

export interface CaptureController<C extends GuidanceCode, M> {
  readonly mode: CaptureMode;
  readonly status: CaptureStatus;
  /** The latest guidance, or null before the first analysed frame. */
  readonly guidance: GuidanceState<C, M> | null;
  /** Where the analysis runs, once started. */
  readonly engine: EngineKind | null;
  /** Whether the preview is mirrored (the captured image never is). */
  readonly mirrored: boolean;
  /** The video element the camera plays in. */
  readonly video: HTMLVideoElement;
  /** Document card guide in frame pixels (null for selfies, or before the first frame). */
  readonly guide: Rect | null;
  /**
   * Opens the camera (the browser may ask for permission), loads the model, starts guidance.
   * Calling it again after a capture or `stop()` resumes with the already loaded model.
   */
  start(): Promise<void>;
  /**
   * Captures now: the sharpest frame of the current still run when there is one, otherwise the
   * latest analysed frame (its issues are reported in `guidance`).
   */
  capture(): Promise<CaptureResult<C, M>>;
  /** Releases the camera and pauses guidance; the model stays loaded. */
  stop(): void;
  /** Releases everything (camera, worker, model). The controller cannot be used afterwards. */
  dispose(): void;
  on<K extends keyof CaptureEventMap<C, M>>(type: K, listener: (value: CaptureEventMap<C, M>[K]) => void): () => void;
}

export interface CommonCaptureOptions {
  /** Video element to play the camera in (the preview). Required: it is also the frame source. */
  readonly video: HTMLVideoElement;
  /** Self-hosted asset locations. Nothing is loaded from a CDN. */
  readonly assets?: AssetOptions;
  /** Thresholds from a service profile (`GET /v1/profiles/{name}` via your backend). Default: `standard`. */
  readonly profile?: ProfileLike;
  /** Per-check overrides applied on top of the profile. */
  readonly checks?: CheckOverrides;
  /** Capture automatically once still and passing. Default true. */
  readonly autoCapture?: boolean;
  readonly stability?: Partial<StabilityOptions>;
  /** Extra frames returned with the capture for server-side replay analysis, 0-10. Default 0. */
  readonly burstCount?: number;
  /** JPEG quality, (0, 1]. Default 0.92. */
  readonly jpegQuality?: number;
  /** Upper bound on analysed frames per second. Default 15. */
  readonly maxAnalysisFps?: number;
  /** Hint smoothing window, ms. Default 400; 0 disables smoothing. */
  readonly hintWindowMs?: number;
  /** Keep the camera on after a capture (for an instant retake). Default false. */
  readonly keepCameraOnCapture?: boolean;
  /** A specific camera (from enumerateDevices). */
  readonly deviceId?: string;
  /** Extra video constraints, merged over the defaults. */
  readonly videoConstraints?: MediaTrackConstraints;
  /** Mirror the preview. Default: selfie true (unless the camera is rear-facing), document false. */
  readonly mirrorPreview?: boolean;
  /** MediaPipe delegate. Default 'CPU' (the most predictable across devices). */
  readonly delegate?: Delegate;
  /** How long the model may take to load, ms. Default 60000. */
  readonly initTimeoutMs?: number;
}

export interface SelfieCaptureOptions extends CommonCaptureOptions {
  /**
   * SDK-only "too close" bound: inter-eye distance / shorter frame side. Default 0.35; null disables.
   */
  readonly maxInterEyeRatio?: number | null;
}

export interface DocumentCaptureOptions extends CommonCaptureOptions {
  /** Optional SDK-only "too close" bound for the portrait. Default null (off). */
  readonly maxInterEyeRatio?: number | null;
  /**
   * Guide until the portrait's inter-eye distance reaches the profile minimum times this factor,
   * so captures clear the service's iris-based check despite the detector's ±8% difference.
   * Default 1.15; 1 guides to the profile minimum exactly. Range [1, 2].
   */
  readonly interEyeDistanceMargin?: number;
  /** Share of the frame the card guide fills, (0, 1]. Default 0.85. */
  readonly guideFill?: number;
}

export type SelfieGuidanceState = GuidanceState<SelfieGuidanceCode, SelfieMeasurements>;
export type DocumentGuidanceState = GuidanceState<DocumentGuidanceCode, DocumentMeasurements>;
export type SelfieCaptureResult = CaptureResult<SelfieGuidanceCode, SelfieMeasurements>;
export type DocumentCaptureResult = CaptureResult<DocumentGuidanceCode, DocumentMeasurements>;
export type SelfieCaptureController = CaptureController<SelfieGuidanceCode, SelfieMeasurements>;
export type DocumentCaptureController = CaptureController<DocumentGuidanceCode, DocumentMeasurements>;
