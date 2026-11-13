/**
 * Capture-session state machine, driven frame by frame with fakes for the camera, the analysis
 * engine, the frame pump and the JPEG encoder. Every fake frame records whether it was closed, so
 * the tests also prove that no full-resolution frame leaks and none is used after it was released.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { FrameAnalysis } from '../src/analysis/types';
import { createSelfieCapture } from '../src/capture/selfie';
import type { SelfieCaptureController, SelfieCaptureOptions, SelfieCaptureResult } from '../src/capture/types';
import type { FaceCaptureError } from '../src/errors';
import type { SelfieMeasurements } from '../src/guidance/selfie';
import { selfieFace, selfieMeasurements } from './helpers';

interface FakeBitmap {
  width: number;
  height: number;
  closed: boolean;
  closeCount: number;
  close(): void;
}

interface FakeTrack {
  readyState: 'live' | 'ended';
  label: string;
  stopped: boolean;
  listeners: Map<string, () => void>;
  getSettings(): MediaTrackSettings;
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string): void;
  stop(): void;
}

const h = vi.hoisted(() => ({
  bitmaps: [] as FakeBitmap[],
  pumps: [] as { callback: (now: number, meta: unknown) => void; running: boolean }[],
  tracks: [] as FakeTrack[],
  next: [] as FrameAnalysis[],
  analyzeError: null as Error | null,
  engineDisposed: 0,
  openCamera: null as null | (() => Promise<unknown>),
}));

vi.mock('../src/analysis/engine', () => ({
  createEngine: () =>
    Promise.resolve({
      kind: 'worker',
      fallbackReason: null,
      analyze(frame: FakeBitmap) {
        if (h.analyzeError) {
          frame.close();
          return Promise.reject(h.analyzeError);
        }
        // Consume queued analyses in order; the last one repeats.
        const analysis = h.next.length > 1 ? h.next.shift() : h.next[0];
        if (!analysis) throw new Error('no analysis queued');
        return Promise.resolve({ analysis, frame });
      },
      dispose() {
        h.engineDisposed += 1;
      },
    }),
}));

vi.mock('../src/capture/frame-pump', () => ({
  FramePump: class {
    readonly source = 'requestVideoFrameCallback';
    running = false;
    constructor(
      _video: unknown,
      readonly callback: (now: number, meta: unknown) => void,
    ) {
      h.pumps.push(this);
    }
    start() {
      this.running = true;
    }
    stop() {
      this.running = false;
    }
  },
}));

vi.mock('../src/capture/encode', () => ({
  DEFAULT_JPEG_QUALITY: 0.92,
  encodeJpeg: (bitmap: FakeBitmap) => {
    if (bitmap.closed) return Promise.reject(new Error('encoded a released frame'));
    return Promise.resolve(new Blob([new Uint8Array([0xff, 0xd8, 0xff])], { type: 'image/jpeg' }));
  },
}));

vi.mock('../src/capture/camera', () => ({
  assertCaptureSupport: () => undefined,
  buildVideoConstraints: () => ({}),
  attachStream: () => Promise.resolve(),
  stopStream: (stream: { getTracks(): FakeTrack[] } | null) => {
    for (const track of stream?.getTracks() ?? []) track.stop();
  },
  openCamera: () => (h.openCamera ? h.openCamera() : Promise.resolve(makeStream())),
}));

function makeStream() {
  const track: FakeTrack = {
    readyState: 'live',
    label: 'Fake camera',
    stopped: false,
    listeners: new Map(),
    getSettings: () => ({ width: 1280, height: 720, frameRate: 30, facingMode: 'user', deviceId: 'secret-id' }),
    addEventListener(type, listener) {
      this.listeners.set(type, listener);
    },
    removeEventListener(type) {
      this.listeners.delete(type);
    },
    stop() {
      this.stopped = true;
      this.readyState = 'ended';
    },
  };
  h.tracks.push(track);
  return {
    get active() {
      return track.readyState === 'live';
    },
    getVideoTracks: () => [track],
    getTracks: () => [track],
  };
}

function analysis(m: SelfieMeasurements): FrameAnalysis {
  return { mode: 'selfie', measurements: m, timing: { detectMs: 10, pixelsMs: 2, totalMs: 12 } };
}

const GOOD = analysis(selfieMeasurements());
const TURNED = analysis(selfieMeasurements(selfieFace({ yaw: 30 })));

/** Lets queued promise callbacks and macrotasks run. */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await new Promise((resolve) => setTimeout(resolve, 0));
}

let now = 1000;
/** Delivers one camera frame (66 ms apart, ~15 fps) and waits for its analysis. */
async function frame(): Promise<void> {
  now += 67;
  const pump = h.pumps.at(-1);
  if (pump?.running) pump.callback(now, { mediaTime: now / 1000, presentedFrames: now });
  await settle();
}

function make(options: Partial<SelfieCaptureOptions> = {}): SelfieCaptureController {
  const video = { style: { transform: '' }, srcObject: null } as unknown as HTMLVideoElement;
  return createSelfieCapture({ video, hintWindowMs: 0, ...options });
}

beforeEach(() => {
  h.bitmaps.length = 0;
  h.pumps.length = 0;
  h.tracks.length = 0;
  h.next.length = 0;
  h.analyzeError = null;
  h.engineDisposed = 0;
  h.openCamera = null;
  vi.stubGlobal('location', { href: 'https://portal.example.com/visit' });
  vi.stubGlobal('createImageBitmap', () => {
    const bitmap: FakeBitmap = {
      width: 1280,
      height: 720,
      closed: false,
      closeCount: 0,
      close() {
        this.closed = true;
        this.closeCount += 1;
      },
    };
    h.bitmaps.push(bitmap);
    return Promise.resolve(bitmap);
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function expectNoLeaks(): void {
  expect(h.bitmaps.length).toBeGreaterThan(0);
  expect(h.bitmaps.filter((b) => b.closeCount !== 1)).toEqual([]);
}

describe('capture session', () => {
  it('auto-captures the sharpest still frame, then releases the camera and every frame', async () => {
    const capture = make({ burstCount: 2 });
    const statuses: string[] = [];
    const codes: string[] = [];
    let result: SelfieCaptureResult | null = null;
    capture.on('status', (s) => statuses.push(s));
    capture.on('guidance', (g) => codes.push(g.code));
    capture.on('capture', (r) => {
      result = r;
    });

    await capture.start();
    expect(capture.status).toBe('running');
    expect(capture.mirrored).toBe(true);
    expect(capture.video.style.transform).toBe('scaleX(-1)');

    // Sharpness varies: frame 3 is the sharpest of the still run.
    for (const raw of [0.5, 0.6, 0.95, 0.55, 0.52, 0.51, 0.5, 0.49, 0.48, 0.47, 0.46]) {
      h.next.push(analysis(selfieMeasurements(selfieFace({ sharpnessRaw: raw }))));
    }
    for (let i = 0; i < 11 && capture.status === 'running'; i++) await frame();

    expect(statuses).toEqual(['starting', 'running', 'capturing', 'captured']);
    expect(codes.at(-1)).toBe('READY');
    expect(result).not.toBeNull();
    const r = result as unknown as SelfieCaptureResult;
    expect(r.blob.type).toBe('image/jpeg');
    expect([r.width, r.height]).toEqual([1280, 720]);
    expect(r.guidance).toMatchObject({ trigger: 'auto', ready: true });
    expect(r.guidance.measurements.face?.sharpnessRaw).toBe(0.95);
    // Capture fires on frame 9 (603 ms of stillness); the burst is frames 8 and 9, timed
    // relative to the sharpest frame (frame 2).
    expect(r.burst.map((b) => b.offsetMs)).toEqual([67 * 6, 67 * 7]);
    expect(r.signals.camera).toEqual({
      label: 'Fake camera',
      settings: { width: 1280, height: 720, frameRate: 30, facingMode: 'user' },
    });
    expect(r.signals.analysis).toMatchObject({ engine: 'worker', framesAnalysed: 10 });
    expect(h.tracks[0]?.stopped).toBe(true);
    expectNoLeaks();
  });

  it('keeps guiding while a check fails, and a manual capture reports the issue', async () => {
    const capture = make();
    await capture.start();
    h.next.push(TURNED);
    for (let i = 0; i < 12; i++) await frame();
    expect(capture.status).toBe('running');
    expect(capture.guidance?.code).toBe('YAW_OUT_OF_RANGE');
    expect(capture.guidance?.issues[0]).toMatchObject({ checkId: 'HEAD_POSE_YAW', direction: 'right' });

    const result = await capture.capture();
    expect(result.guidance).toMatchObject({ trigger: 'manual', ready: false });
    expect(result.guidance.issues.map((i) => i.code)).toEqual(['YAW_OUT_OF_RANGE']);
    expect(capture.status).toBe('captured');
    expectNoLeaks();
  });

  it('can retake after a capture with a fresh camera stream', async () => {
    const capture = make();
    await capture.start();
    h.next.push(GOOD);
    for (let i = 0; i < 12 && capture.status === 'running'; i++) await frame();
    expect(capture.status).toBe('captured');

    await capture.start();
    expect(capture.status).toBe('running');
    expect(h.tracks).toHaveLength(2);
    expect(capture.guidance).toBeNull();
    for (let i = 0; i < 12 && capture.status === 'running'; i++) await frame();
    expect(capture.status).toBe('captured');
    expectNoLeaks();
  });

  it('resumes on the open camera with keepCameraOnCapture', async () => {
    const capture = make({ keepCameraOnCapture: true });
    await capture.start();
    h.next.push(GOOD);
    for (let i = 0; i < 12 && capture.status === 'running'; i++) await frame();
    expect(capture.status).toBe('captured');
    expect(h.tracks[0]?.stopped).toBe(false);
    await capture.start();
    expect(capture.status).toBe('running');
    expect(h.tracks).toHaveLength(1);
    capture.dispose();
    expect(h.tracks[0]?.stopped).toBe(true);
  });

  it('reports a camera that stops delivering video', async () => {
    const capture = make();
    const errors: FaceCaptureError[] = [];
    capture.on('error', (e) => errors.push(e));
    await capture.start();
    h.tracks[0]?.listeners.get('ended')?.();
    expect(capture.status).toBe('error');
    expect(errors.map((e) => e.code)).toEqual(['camera-unavailable']);
  });

  it('refuses a stream whose track has already ended instead of waiting forever', async () => {
    h.openCamera = () => {
      const stream = makeStream();
      stream.getVideoTracks()[0]?.stop();
      return Promise.resolve(stream);
    };
    const capture = make();
    await expect(capture.start()).rejects.toMatchObject({ code: 'camera-unavailable' });
    expect(capture.status).toBe('error');
  });

  it('ends with an error event when analysis fails', async () => {
    const capture = make();
    const errors: FaceCaptureError[] = [];
    capture.on('error', (e) => errors.push(e));
    await capture.start();
    h.next.push(GOOD);
    await frame();
    const { FaceCaptureError } = await import('../src/errors');
    h.analyzeError = new FaceCaptureError('internal', 'model crashed');
    await frame();
    expect(capture.status).toBe('error');
    expect(errors.map((e) => e.code)).toEqual(['internal']);
    expect(h.engineDisposed).toBe(1);
    expectNoLeaks();
  });

  it('stays quiet when stopped while the camera is still opening', async () => {
    let open: (stream: unknown) => void = () => undefined;
    h.openCamera = () =>
      new Promise((resolve) => {
        open = resolve;
      });
    const capture = make();
    const errors: FaceCaptureError[] = [];
    capture.on('error', (e) => errors.push(e));
    const starting = capture.start();
    await settle();
    capture.stop();
    open(makeStream());
    await starting;
    expect(capture.status).toBe('stopped');
    expect(errors).toEqual([]);
    expect(h.tracks[0]?.stopped).toBe(true);
  });

  it('rejects capture() before start() and everything after dispose()', async () => {
    const capture = make();
    await expect(capture.capture()).rejects.toMatchObject({ code: 'not-running' });
    capture.dispose();
    expect(capture.status).toBe('disposed');
    await expect(capture.start()).rejects.toMatchObject({ code: 'disposed' });
  });

  it('rejects invalid options up front', () => {
    expect(() => make({ burstCount: 50 })).toThrow(/burstCount/);
    expect(() => make({ checks: { HEAD_POSE_YAW: { min: 10, max: -10 } } })).toThrow(/HEAD_POSE_YAW/);
  });
});
