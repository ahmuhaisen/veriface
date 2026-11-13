import { FaceCaptureError } from '../errors';

/**
 * Where the self-hosted runtime assets live. Nothing is loaded from a CDN: by default every file is
 * served from `/face-capture/` on the page's own origin. See the README for the files to copy.
 */
export interface AssetOptions {
  /** Base URL of all assets. Default `/face-capture/`, resolved against the page URL. */
  readonly baseUrl?: string;
  /** Directory with the MediaPipe Tasks Vision Wasm files. Default `<baseUrl>wasm/`. */
  readonly wasmBaseUrl?: string;
  /** Face landmarker model (selfie). Default `<baseUrl>models/face_landmarker.task`. */
  readonly faceLandmarkerModelUrl?: string;
  /** Face detector model (document). Default `<baseUrl>models/blaze_face_short_range.tflite`. */
  readonly faceDetectorModelUrl?: string;
  /**
   * Analysis worker script, which must be same-origin with the page. Default
   * `<baseUrl>face-capture.worker.js`. `null` runs the analysis on the main thread.
   */
  readonly workerUrl?: string | null;
  /** Expected SHA-256 (hex) of the landmarker model. Default: the pinned upstream file. `null` skips the check. */
  readonly faceLandmarkerModelSha256?: string | null;
  /** Expected SHA-256 (hex) of the detector model. Default: the pinned upstream file. `null` skips the check. */
  readonly faceDetectorModelSha256?: string | null;
}

export const DEFAULT_ASSET_BASE_URL = '/face-capture/';

/** File layout under the asset base URL. */
export const ASSET_PATHS = {
  worker: 'face-capture.worker.js',
  wasmDirectory: 'wasm',
  faceLandmarkerModel: 'models/face_landmarker.task',
  faceDetectorModel: 'models/blaze_face_short_range.tflite',
} as const;

/**
 * SHA-256 of the upstream MediaPipe model files the SDK is tested with (Apache-2.0):
 * - face_landmarker/face_landmarker/float16/1/face_landmarker.task
 * - face_detector/blaze_face_short_range/float16/1/blaze_face_short_range.tflite
 * from https://storage.googleapis.com/mediapipe-models/ (download them once, then self-host).
 */
export const MODEL_SHA256 = {
  faceLandmarker: '64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff',
  faceDetector: 'b4578f35940bf5a1a655214a1cce5cab13eba73c1297cd78e1a04c2380b0152f',
} as const;

export interface ResolvedAssets {
  /** Absolute, without a trailing slash (MediaPipe appends `/<file>`). */
  readonly wasmBaseUrl: string;
  readonly faceLandmarkerModelUrl: string;
  readonly faceDetectorModelUrl: string;
  readonly workerUrl: string | null;
  readonly faceLandmarkerModelSha256: string | null;
  readonly faceDetectorModelSha256: string | null;
  /** Origins of all asset URLs; the worker refuses requests to any other origin. */
  readonly origins: readonly string[];
}

/** Resolves asset URLs against the page URL (`document.baseURI`). */
export function resolveAssets(options: AssetOptions | undefined, pageUrl: string): ResolvedAssets {
  const base = new URL(withTrailingSlash(options?.baseUrl ?? DEFAULT_ASSET_BASE_URL), pageUrl);
  const at = (value: string | undefined, fallback: string) => new URL(value ?? fallback, base).href;
  const wasmBaseUrl = withoutTrailingSlash(at(options?.wasmBaseUrl, ASSET_PATHS.wasmDirectory));
  const faceLandmarkerModelUrl = at(options?.faceLandmarkerModelUrl, ASSET_PATHS.faceLandmarkerModel);
  const faceDetectorModelUrl = at(options?.faceDetectorModelUrl, ASSET_PATHS.faceDetectorModel);
  const workerUrl = options?.workerUrl === null ? null : at(options?.workerUrl, ASSET_PATHS.worker);
  const urls = [wasmBaseUrl, faceLandmarkerModelUrl, faceDetectorModelUrl, ...(workerUrl ? [workerUrl] : [])];
  return {
    wasmBaseUrl,
    faceLandmarkerModelUrl,
    faceDetectorModelUrl,
    workerUrl,
    faceLandmarkerModelSha256: checksumOption(options?.faceLandmarkerModelSha256, MODEL_SHA256.faceLandmarker),
    faceDetectorModelSha256: checksumOption(options?.faceDetectorModelSha256, MODEL_SHA256.faceDetector),
    origins: [...new Set(urls.map((u) => new URL(u).origin))],
  };
}

function checksumOption(value: string | null | undefined, fallback: string): string | null {
  if (value === null) return null;
  const hex = (value ?? fallback).trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(hex)) {
    throw new FaceCaptureError('invalid-options', 'A model SHA-256 must be 64 hexadecimal characters');
  }
  return hex;
}

function withTrailingSlash(url: string): string {
  return url.endsWith('/') ? url : `${url}/`;
}

function withoutTrailingSlash(url: string): string {
  return url.replace(/\/+$/, '');
}

export function toHex(buffer: ArrayBuffer): string {
  return Array.from(new Uint8Array(buffer), (b) => b.toString(16).padStart(2, '0')).join('');
}

/**
 * Downloads a model file and verifies its SHA-256 when one is given, so a tampered or corrupted
 * model is refused before MediaPipe sees it.
 */
export async function fetchModel(url: string, sha256: string | null): Promise<Uint8Array<ArrayBuffer>> {
  let response: Response;
  try {
    response = await fetch(url, { credentials: 'same-origin' });
  } catch (error) {
    throw new FaceCaptureError('asset-load-failed', `Could not load the model ${url}`, { cause: error });
  }
  if (!response.ok) {
    throw new FaceCaptureError('asset-load-failed', `Could not load the model ${url} (HTTP ${response.status})`);
  }
  const buffer = await response.arrayBuffer();
  if (sha256 !== null) {
    const actual = toHex(await crypto.subtle.digest('SHA-256', buffer));
    if (actual !== sha256) {
      throw new FaceCaptureError(
        'asset-load-failed',
        `Model integrity check failed for ${url}: expected SHA-256 ${sha256}, got ${actual}`,
      );
    }
  }
  return new Uint8Array(buffer);
}
