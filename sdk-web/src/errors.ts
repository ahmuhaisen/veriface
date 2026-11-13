/** Machine-readable error codes. User-facing text comes from the `error.*` message keys. */
export type FaceCaptureErrorCode =
  /** The browser lacks a required API (getUserMedia, createImageBitmap, WebAssembly, ...). */
  | 'unsupported'
  /** The page is not a secure context (HTTPS or localhost), so the camera is unavailable. */
  | 'insecure-context'
  /** The user or a policy denied camera access. */
  | 'camera-permission-denied'
  /** No camera matches the request. */
  | 'camera-not-found'
  /** A camera exists but could not be started or stopped working (in use, unplugged, revoked). */
  | 'camera-unavailable'
  /** A self-hosted asset (Wasm, model, worker) could not be loaded or failed its integrity check. */
  | 'asset-load-failed'
  /** The self-hosted worker file belongs to a different SDK version; re-copy the assets. */
  | 'asset-version-mismatch'
  /** An option or threshold is invalid. */
  | 'invalid-options'
  /** The operation needs a running capture (for example `capture()` before `start()`). */
  | 'not-running'
  /** The capture was disposed. */
  | 'disposed'
  /** An unexpected internal failure. */
  | 'internal';

export class FaceCaptureError extends Error {
  override readonly name = 'FaceCaptureError';
  readonly code: FaceCaptureErrorCode;

  constructor(code: FaceCaptureErrorCode, message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.code = code;
  }
}

export function isFaceCaptureError(value: unknown): value is FaceCaptureError {
  return value instanceof FaceCaptureError;
}

/** Wraps any thrown value in a FaceCaptureError, keeping an existing code. */
export function toFaceCaptureError(
  value: unknown,
  fallback: FaceCaptureErrorCode = 'internal',
): FaceCaptureError {
  if (value instanceof FaceCaptureError) return value;
  const message = value instanceof Error ? value.message : String(value);
  return new FaceCaptureError(fallback, message, { cause: value });
}

/** Maps a getUserMedia failure (a DOMException name) to an error code. */
export function cameraErrorCode(name: string | undefined): FaceCaptureErrorCode {
  switch (name) {
    case 'NotAllowedError':
    case 'PermissionDeniedError':
      return 'camera-permission-denied';
    case 'NotFoundError':
    case 'DevicesNotFoundError':
    case 'OverconstrainedError':
    case 'ConstraintNotSatisfiedError':
      return 'camera-not-found';
    case 'NotReadableError':
    case 'TrackStartError':
    case 'AbortError':
      return 'camera-unavailable';
    case 'SecurityError':
      return 'insecure-context';
    case 'TypeError':
    case 'NotSupportedError':
      return 'unsupported';
    default:
      return 'camera-unavailable';
  }
}

export function toCameraError(value: unknown): FaceCaptureError {
  if (value instanceof FaceCaptureError) return value;
  const name = value instanceof Error ? value.name : undefined;
  const message = value instanceof Error ? value.message : String(value);
  return new FaceCaptureError(cameraErrorCode(name), message || 'Camera error', { cause: value });
}
