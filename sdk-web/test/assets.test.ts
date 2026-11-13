import { afterEach, describe, expect, it, vi } from 'vitest';
import { fetchModel, MODEL_SHA256, resolveAssets, toHex } from '../src/analysis/assets';
import { installFetchGuard, isAllowedRequest } from '../src/analysis/fetch-guard';
import { FaceCaptureError } from '../src/errors';

const PAGE = 'https://portal.example.com/visit/selfie?step=2';

describe('asset resolution (self-hosted, no CDN)', () => {
  it('defaults to /face-capture/ on the page origin', () => {
    const assets = resolveAssets(undefined, PAGE);
    expect(assets.wasmBaseUrl).toBe('https://portal.example.com/face-capture/wasm');
    expect(assets.faceLandmarkerModelUrl).toBe('https://portal.example.com/face-capture/models/face_landmarker.task');
    expect(assets.faceDetectorModelUrl).toBe(
      'https://portal.example.com/face-capture/models/blaze_face_short_range.tflite',
    );
    expect(assets.workerUrl).toBe('https://portal.example.com/face-capture/face-capture.worker.js');
    expect(assets.origins).toEqual(['https://portal.example.com']);
    expect(assets.faceLandmarkerModelSha256).toBe(MODEL_SHA256.faceLandmarker);
    expect(assets.faceDetectorModelSha256).toBe(MODEL_SHA256.faceDetector);
  });

  it('resolves a relative base URL against the page', () => {
    const assets = resolveAssets({ baseUrl: 'static/fc' }, PAGE);
    expect(assets.workerUrl).toBe('https://portal.example.com/visit/static/fc/face-capture.worker.js');
    expect(assets.wasmBaseUrl).toBe('https://portal.example.com/visit/static/fc/wasm');
  });

  it('accepts individual overrides, strips the Wasm trailing slash and collects origins', () => {
    const assets = resolveAssets(
      {
        wasmBaseUrl: 'https://assets.example.net/mp/wasm/',
        faceLandmarkerModelUrl: '/models/custom.task',
        workerUrl: null,
        faceLandmarkerModelSha256: null,
      },
      PAGE,
    );
    expect(assets.wasmBaseUrl).toBe('https://assets.example.net/mp/wasm');
    expect(assets.faceLandmarkerModelUrl).toBe('https://portal.example.com/models/custom.task');
    expect(assets.workerUrl).toBeNull();
    expect(assets.faceLandmarkerModelSha256).toBeNull();
    expect([...assets.origins].sort()).toEqual(['https://assets.example.net', 'https://portal.example.com']);
  });

  it('rejects a malformed SHA-256', () => {
    expect(() => resolveAssets({ faceDetectorModelSha256: 'abc' }, PAGE)).toThrow(FaceCaptureError);
  });

  it('normalises SHA-256 case', () => {
    const upper = MODEL_SHA256.faceDetector.toUpperCase();
    expect(resolveAssets({ faceDetectorModelSha256: upper }, PAGE).faceDetectorModelSha256).toBe(
      MODEL_SHA256.faceDetector,
    );
  });
});

describe('model download with integrity check', () => {
  const bytes = new TextEncoder().encode('not really a model');
  const digest = async () => toHex(await crypto.subtle.digest('SHA-256', bytes));

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('returns the bytes when the SHA-256 matches', async () => {
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(bytes))));
    const model = await fetchModel('https://x.test/m.task', await digest());
    expect(Array.from(model)).toEqual(Array.from(bytes));
  });

  it('skips the check when no SHA-256 is configured', async () => {
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(bytes))));
    await expect(fetchModel('https://x.test/m.task', null)).resolves.toHaveLength(bytes.length);
  });

  it.each([
    ['a tampered file', () => Promise.resolve(new Response(new TextEncoder().encode('tampered')))],
    ['an HTTP error', () => Promise.resolve(new Response('missing', { status: 404 }))],
    ['a network failure', () => Promise.reject(new TypeError('Failed to fetch'))],
  ])('refuses %s with asset-load-failed', async (_name, impl) => {
    vi.stubGlobal('fetch', vi.fn(impl));
    const sha = await digest();
    await expect(fetchModel('https://x.test/m.task', sha)).rejects.toMatchObject({ code: 'asset-load-failed' });
  });

  it('formats digests as lowercase hex', () => {
    expect(toHex(new Uint8Array([0, 15, 16, 255]).buffer)).toBe('000f10ff');
  });
});

describe('worker network allow-list', () => {
  const allowed = ['https://portal.example.com'];

  it.each([
    ['https://portal.example.com/face-capture/wasm/vision_wasm_internal.wasm', true],
    ['/face-capture/models/face_landmarker.task', true],
    ['blob:https://portal.example.com/1234', true],
    ['data:application/octet-stream;base64,AAAA', true],
    ['https://odml.pa.googleapis.com/v1/log', false],
    ['https://cdn.jsdelivr.net/npm/@mediapipe/tasks-vision/wasm', false],
    ['http://portal.example.com/plain-http', false],
    ['https://portal.example.com.evil.test/x', false],
  ])('%s -> %s', (url, expected) => {
    expect(isAllowedRequest(url, allowed, 'https://portal.example.com/face-capture/face-capture.worker.js')).toBe(
      expected,
    );
  });

  it('blocks disallowed requests before they reach the network', async () => {
    const original = vi.fn(() => Promise.resolve(new Response('ok')));
    const scope = { fetch: original as typeof fetch, location: { href: 'https://portal.example.com/w.js' } };
    installFetchGuard(scope, allowed);
    await expect(scope.fetch('https://odml.pa.googleapis.com/v1/log', { method: 'POST' })).rejects.toThrow(/blocked/);
    expect(original).not.toHaveBeenCalled();
    await scope.fetch('/face-capture/models/face_landmarker.task');
    await scope.fetch(new URL('https://portal.example.com/a'));
    await scope.fetch(new Request('https://portal.example.com/b'));
    expect(original).toHaveBeenCalledTimes(3);
  });
});
