import { describe, expect, it } from 'vitest';
import { StabilityTracker, type StabilitySample } from '../src/guidance/stability';
import { DEFAULT_SELFIE_STABILITY, type StabilityOptions } from '../src/thresholds';
import { RefLedger } from './helpers';

const options: StabilityOptions = { ...DEFAULT_SELFIE_STABILITY, windowMs: 600, minFrames: 3 };
const FRAME_MS = 1000 / 15;

function sample(i: number, overrides: Partial<StabilitySample> = {}): StabilitySample {
  return {
    timestamp: 1000 + i * FRAME_MS,
    passed: true,
    position: { x: 640, y: 330 },
    scale: 150,
    angles: [0, 0, 0],
    sharpness: 0.5,
    ...overrides,
  };
}

function tracker(ledger = new RefLedger()) {
  return { ledger, t: new StabilityTracker<number>(options, ledger.retain, ledger.release) };
}

describe('stability window', () => {
  it('becomes ready after windowMs of still, passing frames', () => {
    const { t } = tracker();
    const states = Array.from({ length: 12 }, (_, i) => t.push(sample(i), i));
    const firstReady = states.findIndex((s) => s.ready);
    // 600 ms at 15 fps: the 10th frame (index 9) is 600 ms after the first.
    expect(firstReady).toBe(9);
    expect(states[8]?.ready).toBe(false);
    expect(states[8]?.progress).toBeGreaterThan(0.85);
    expect(states[9]?.progress).toBe(1);
  });

  it('also needs minFrames', () => {
    const slow = new StabilityTracker<number>({ ...options, minFrames: 5 });
    const states = [0, 400, 800].map((ms, i) => slow.push(sample(0, { timestamp: 1000 + ms }), i));
    expect(states.at(-1)?.stableMs).toBe(800);
    expect(states.at(-1)?.ready).toBe(false);
  });

  it('restarts the run when a frame fails a check', () => {
    const { t } = tracker();
    for (let i = 0; i < 8; i++) t.push(sample(i), i);
    expect(t.push(sample(8, { passed: false }), 8)).toMatchObject({ stable: false, ready: false, frames: 0 });
    expect(t.best()).toBeNull();
    expect(t.push(sample(9), 9)).toMatchObject({ frames: 1, stableMs: 0 });
  });

  it('restarts the run when the face moves too fast, and reports movement', () => {
    const { t } = tracker();
    for (let i = 0; i < 5; i++) t.push(sample(i), i);
    // 30 px in one 67 ms frame at an inter-eye distance of 150 px: 3 IED/s, above 0.5.
    const moved = t.push(sample(5, { position: { x: 670, y: 330 } }), 5);
    expect(moved).toMatchObject({ moving: true, stable: false, frames: 1 });
  });

  it('tolerates small jitter', () => {
    const { t } = tracker();
    let state = t.push(sample(0), 0);
    for (let i = 1; i < 12; i++) state = t.push(sample(i, { position: { x: 640 + (i % 2), y: 330 } }), i);
    expect(state.ready).toBe(true);
  });

  it('restarts the run on a fast change of scale or head angle', () => {
    const { t } = tracker();
    for (let i = 0; i < 5; i++) t.push(sample(i), i);
    expect(t.push(sample(5, { scale: 170 }), 5).moving).toBe(true);
    for (let i = 6; i < 10; i++) t.push(sample(i, { scale: 170 }), i);
    expect(t.push(sample(10, { scale: 170, angles: [5, 0, 0] }), 10).moving).toBe(true);
  });

  it('restarts after a long gap between frames (for example, a hidden tab)', () => {
    const { t } = tracker();
    for (let i = 0; i < 5; i++) t.push(sample(i), i);
    const after = t.push(sample(5, { timestamp: 1000 + 5 * FRAME_MS + 2000 }), 5);
    expect(after).toMatchObject({ frames: 1, moving: false });
  });

  it('still captures on a slow device that analyses about one frame a second', () => {
    const { t } = tracker();
    const states = [0, 1002, 2004].map((ms, i) => t.push(sample(0, { timestamp: 1000 + ms }), i));
    expect(states.map((s) => s.frames)).toEqual([1, 2, 3]);
    expect(states.at(-1)?.ready).toBe(true);
  });
});

describe('best-frame selection', () => {
  it('picks the sharpest frame of the window', () => {
    const { t } = tracker();
    const sharpness = [0.4, 0.45, 0.9, 0.5, 0.6, 0.55, 0.52, 0.51, 0.5, 0.49];
    sharpness.forEach((s, i) => t.push(sample(i, { sharpness: s }), i));
    expect(t.best()?.payload).toBe(2);
    expect(t.best()?.sample.sharpness).toBe(0.9);
  });

  it('forgets frames older than the window', () => {
    const { t } = tracker();
    t.push(sample(0, { sharpness: 0.95 }), 0);
    for (let i = 1; i < 12; i++) t.push(sample(i, { sharpness: 0.5 + i * 0.001 }), i);
    // Frame 0 is now 733 ms old; the newest, sharpest of the rest wins.
    expect(t.best()?.payload).toBe(11);
  });

  it('never selects a frame without a sharpness value', () => {
    const { t } = tracker();
    t.push(sample(0, { sharpness: null }), 0);
    expect(t.best()).toBeNull();
    t.push(sample(1, { sharpness: 0.1 }), 1);
    expect(t.best()?.payload).toBe(1);
  });

  it('releases every payload it retained exactly once (no leaked frames)', () => {
    const { t, ledger } = tracker();
    for (let i = 0; i < 40; i++) {
      const s = sample(i, {
        sharpness: (i * 7919) % 13 / 13,
        passed: i % 11 !== 0,
        position: { x: 640 + (i % 17 === 0 ? 100 : 0), y: 330 },
      });
      t.push(s, i);
      // Bounded memory: a sliding-window maximum keeps few candidates.
      expect(ledger.held().length).toBeLessThanOrEqual(10);
    }
    t.reset();
    expect(ledger.held()).toEqual([]);
    expect(ledger.overReleased()).toEqual([]);
  });
});
