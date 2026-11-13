import { describe, expect, it } from 'vitest';
import { cameraErrorCode, FaceCaptureError, toCameraError, toFaceCaptureError } from '../src/errors';

function domError(name: string, message = 'x'): Error {
  const error = new Error(message);
  error.name = name;
  return error;
}

describe('camera errors', () => {
  it.each([
    ['NotAllowedError', 'camera-permission-denied'],
    ['PermissionDeniedError', 'camera-permission-denied'],
    ['NotFoundError', 'camera-not-found'],
    ['OverconstrainedError', 'camera-not-found'],
    ['NotReadableError', 'camera-unavailable'],
    ['AbortError', 'camera-unavailable'],
    ['SecurityError', 'insecure-context'],
    ['NotSupportedError', 'unsupported'],
    ['SomethingNew', 'camera-unavailable'],
  ] as const)('%s -> %s', (name, code) => {
    expect(cameraErrorCode(name)).toBe(code);
    expect(toCameraError(domError(name)).code).toBe(code);
  });

  it('keeps the original error as the cause', () => {
    const original = domError('NotAllowedError', 'Permission denied');
    const error = toCameraError(original);
    expect(error).toBeInstanceOf(FaceCaptureError);
    expect(error.cause).toBe(original);
    expect(error.message).toBe('Permission denied');
  });
});

describe('toFaceCaptureError', () => {
  it('keeps existing codes and wraps anything else', () => {
    const existing = new FaceCaptureError('asset-load-failed', 'missing');
    expect(toFaceCaptureError(existing)).toBe(existing);
    expect(toFaceCaptureError(new Error('boom')).code).toBe('internal');
    expect(toFaceCaptureError('text', 'unsupported')).toMatchObject({ code: 'unsupported', message: 'text' });
  });
});
