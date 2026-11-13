import { describe, expect, it, vi } from 'vitest';
import { FrameRef } from '../src/capture/frame-ref';
import { FrameTimingCollector, RunningStats, snapshotTrackSettings } from '../src/capture/signals';

describe('RunningStats', () => {
  it('matches a direct computation', () => {
    const values = [33.1, 33.4, 32.9, 66.8, 33.3, 33.2];
    const stats = new RunningStats();
    for (const v of values) stats.push(v);
    const mean = values.reduce((a, b) => a + b, 0) / values.length;
    const variance = values.reduce((a, b) => a + (b - mean) ** 2, 0) / (values.length - 1);
    const snapshot = stats.snapshot();
    expect(snapshot?.count).toBe(values.length);
    expect(snapshot?.mean).toBeCloseTo(mean, 10);
    expect(snapshot?.stdDev).toBeCloseTo(Math.sqrt(variance), 10);
    expect(snapshot?.min).toBe(32.9);
    expect(snapshot?.max).toBe(66.8);
  });

  it('ignores non-finite values and reports nothing when empty', () => {
    const stats = new RunningStats();
    expect(stats.snapshot()).toBeNull();
    stats.push(Number.NaN);
    stats.push(Infinity);
    expect(stats.snapshot()).toBeNull();
    stats.push(5);
    expect(stats.snapshot()).toMatchObject({ count: 1, mean: 5, stdDev: 0 });
  });
});

describe('frame timing signals', () => {
  it('uses camera frame timestamps (mediaTime) with requestVideoFrameCallback', () => {
    const timing = new FrameTimingCollector('requestVideoFrameCallback');
    // A 30 fps camera; one callback missed (presentedFrames jumps by 2).
    const frames = [
      { now: 1000, mediaTime: 0, presentedFrames: 10 },
      { now: 1034, mediaTime: 1 / 30, presentedFrames: 11 },
      { now: 1066, mediaTime: 2 / 30, presentedFrames: 12 },
      { now: 1133, mediaTime: 4 / 30, presentedFrames: 14 },
    ];
    for (const f of frames) timing.record(f.now, f);
    const snapshot = timing.snapshot();
    expect(snapshot.basis).toBe('mediaTime');
    expect(snapshot.callbacks).toBe(4);
    expect(snapshot.presentedFrames).toBe(5);
    expect(snapshot.durationMs).toBe(133);
    expect(snapshot.intervalMs?.count).toBe(3);
    expect(snapshot.intervalMs?.min).toBeCloseTo(1000 / 30, 6);
    expect(snapshot.intervalMs?.max).toBeCloseTo(2000 / 30, 6);
  });

  it('falls back to callback intervals without frame metadata', () => {
    const timing = new FrameTimingCollector('requestAnimationFrame');
    [0, 16, 33, 50].forEach((now) => {
      timing.record(now, null);
    });
    const snapshot = timing.snapshot();
    expect(snapshot.basis).toBe('callback');
    expect(snapshot.presentedFrames).toBeNull();
    expect(snapshot.intervalMs?.count).toBe(3);
    expect(snapshot.intervalMs?.mean).toBeCloseTo(50 / 3, 6);
  });
});

describe('camera settings snapshot', () => {
  it('keeps descriptive settings and drops persistent identifiers', () => {
    const snapshot = snapshotTrackSettings({
      deviceId: 'a1b2c3',
      groupId: 'g9',
      width: 1280,
      height: 720,
      frameRate: 30,
      aspectRatio: 1.777,
      facingMode: 'user',
      resizeMode: 'none',
    } as MediaTrackSettings);
    expect(snapshot).toEqual({
      width: 1280,
      height: 720,
      frameRate: 30,
      aspectRatio: 1.777,
      facingMode: 'user',
      resizeMode: 'none',
    });
    expect(JSON.stringify(snapshot)).not.toMatch(/a1b2c3|g9/);
  });
});

describe('FrameRef', () => {
  it('closes the frame once, when the last holder releases it', () => {
    const close = vi.fn();
    const ref = new FrameRef({ close }, 0);
    ref.retain();
    ref.retain();
    ref.release();
    ref.release();
    expect(close).not.toHaveBeenCalled();
    ref.release();
    expect(close).toHaveBeenCalledTimes(1);
    ref.release();
    expect(close).toHaveBeenCalledTimes(1);
    expect(ref.released).toBe(true);
    expect(() => ref.retain()).toThrow();
  });
});
