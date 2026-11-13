import { FaceCaptureError, toCameraError } from '../errors';
import type { CaptureMode } from '../analysis/types';

/** Front camera, ideally 1280x720. */
export const SELFIE_VIDEO_CONSTRAINTS: MediaTrackConstraints = {
  facingMode: { ideal: 'user' },
  width: { ideal: 1280 },
  height: { ideal: 720 },
  frameRate: { ideal: 30 },
};

/**
 * Rear camera preferred, at a higher resolution: the document portrait must reach the profile's
 * inter-eye distance (60 px in `standard`), which needs a card about 800 px wide in the frame.
 */
export const DOCUMENT_VIDEO_CONSTRAINTS: MediaTrackConstraints = {
  facingMode: { ideal: 'environment' },
  width: { ideal: 1920 },
  height: { ideal: 1080 },
  frameRate: { ideal: 30 },
};

export function buildVideoConstraints(
  mode: CaptureMode,
  deviceId?: string,
  overrides?: MediaTrackConstraints,
): MediaTrackConstraints {
  const base = mode === 'selfie' ? SELFIE_VIDEO_CONSTRAINTS : DOCUMENT_VIDEO_CONSTRAINTS;
  const device: MediaTrackConstraints = deviceId ? { deviceId: { exact: deviceId } } : {};
  return { ...base, ...device, ...overrides };
}

/** Throws a FaceCaptureError when the browser cannot capture at all. */
export function assertCaptureSupport(): void {
  if (typeof window !== 'undefined' && !window.isSecureContext) {
    throw new FaceCaptureError('insecure-context', 'Camera access needs a secure context (HTTPS or localhost)');
  }
  // The DOM typings promise mediaDevices, but browsers leave it undefined outside secure contexts.
  const devices = typeof navigator === 'undefined' ? undefined : (navigator as { mediaDevices?: MediaDevices }).mediaDevices;
  if (typeof devices?.getUserMedia !== 'function') {
    throw new FaceCaptureError('unsupported', 'navigator.mediaDevices.getUserMedia is unavailable');
  }
  if (typeof createImageBitmap !== 'function') {
    throw new FaceCaptureError('unsupported', 'createImageBitmap is unavailable');
  }
  if (typeof WebAssembly !== 'object') {
    throw new FaceCaptureError('unsupported', 'WebAssembly is unavailable');
  }
}

export async function openCamera(constraints: MediaTrackConstraints): Promise<MediaStream> {
  try {
    return await navigator.mediaDevices.getUserMedia({ video: constraints, audio: false });
  } catch (error) {
    throw toCameraError(error);
  }
}

export function stopStream(stream: MediaStream | null): void {
  for (const track of stream?.getTracks() ?? []) track.stop();
}

/** Plays the stream in the video element and waits until frame dimensions are known. */
export async function attachStream(video: HTMLVideoElement, stream: MediaStream): Promise<void> {
  video.muted = true;
  video.playsInline = true;
  video.autoplay = true;
  video.setAttribute('playsinline', '');
  video.srcObject = stream;
  if (video.readyState < HTMLMediaElement.HAVE_METADATA) {
    await new Promise<void>((resolve, reject) => {
      const onLoaded = () => {
        cleanup();
        resolve();
      };
      const onError = () => {
        cleanup();
        reject(new FaceCaptureError('camera-unavailable', 'The camera stream could not be played'));
      };
      const cleanup = () => {
        video.removeEventListener('loadedmetadata', onLoaded);
        video.removeEventListener('error', onError);
      };
      video.addEventListener('loadedmetadata', onLoaded);
      video.addEventListener('error', onError);
    });
  }
  try {
    await video.play();
  } catch (error) {
    throw new FaceCaptureError('camera-unavailable', 'The camera preview could not be started', { cause: error });
  }
}
