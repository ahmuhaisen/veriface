import type { FaceCaptureErrorCode } from '../errors';
import type { DocumentMeasurements } from '../guidance/document';
import type { SelfieMeasurements } from '../guidance/selfie';
import type { Rect } from '../measure/geometry';

export type CaptureMode = 'selfie' | 'document';
export type Delegate = 'CPU' | 'GPU';

/** Everything an analyzer needs to load MediaPipe. All URLs are absolute. */
export interface AnalyzerConfig {
  readonly mode: CaptureMode;
  /** Directory holding the MediaPipe Wasm files, without a trailing slash. */
  readonly wasmBaseUrl: string;
  readonly modelUrl: string;
  /** Expected SHA-256 (hex) of the model file, or null to skip the integrity check. */
  readonly modelSha256: string | null;
  readonly delegate: Delegate;
}

export interface AnalysisTiming {
  /** Model inference time, ms. */
  readonly detectMs: number;
  /** Pixel statistics time, ms. */
  readonly pixelsMs: number;
  readonly totalMs: number;
}

/** The result of analysing one frame. Plain data, so it crosses the worker boundary unchanged. */
export type FrameAnalysis =
  | { readonly mode: 'selfie'; readonly measurements: SelfieMeasurements; readonly timing: AnalysisTiming }
  | { readonly mode: 'document'; readonly measurements: DocumentMeasurements; readonly timing: AnalysisTiming };

export interface Analyzer {
  /**
   * Analyses one full-resolution, UN-mirrored frame. `timestamp` must increase from call to call
   * (the analyzer enforces it). `guide` is the document card guide in frame pixels.
   */
  analyze(frame: ImageBitmap, timestamp: number, guide: Rect | null): Promise<FrameAnalysis>;
  close(): void;
}

/** Messages from the main thread to the analysis worker. */
export type WorkerRequest =
  | {
      readonly type: 'init';
      readonly id: number;
      readonly config: AnalyzerConfig;
      /** Origins the worker may fetch from (the self-hosted asset origins). */
      readonly allowedOrigins: readonly string[];
    }
  | {
      readonly type: 'analyze';
      readonly id: number;
      readonly frame: ImageBitmap;
      readonly timestamp: number;
      readonly guide: Rect | null;
    }
  | { readonly type: 'dispose' };

/** Messages from the analysis worker. Frames are transferred back so the main thread can encode them. */
export type WorkerResponse =
  | { readonly type: 'ready'; readonly id: number; readonly version: string }
  | { readonly type: 'result'; readonly id: number; readonly analysis: FrameAnalysis; readonly frame: ImageBitmap }
  | {
      readonly type: 'error';
      readonly id: number;
      readonly code: FaceCaptureErrorCode;
      readonly message: string;
      readonly frame: ImageBitmap | null;
    };
