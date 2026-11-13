import type { Delegate } from '../analysis/types';
import type { EngineKind } from '../analysis/engine';
import type { FrameSource } from './frame-pump';

/**
 * Capture signals: camera and timing facts collected during capture.
 *
 * DETERRENCE AND DIAGNOSTICS ONLY, NOT SECURITY. Everything here is produced by the browser, which
 * the person capturing controls: a modified page, a virtual camera or an injected stream can make
 * these values say anything. Use them to spot oddities, debug devices and support reviews; never
 * accept or reject a person on them. The face service re-checks every image itself.
 */
export interface CaptureSignals {
  readonly sdkVersion: string;
  readonly camera: CameraSignals;
  readonly frameTiming: FrameTimingSignals;
  readonly analysis: AnalysisSignals;
}

export interface CameraSignals {
  /** The track label, e.g. "FaceTime HD Camera" (virtual cameras usually name themselves). */
  readonly label: string;
  /** Selected track settings. Device and group IDs are left out on purpose (persistent identifiers). */
  readonly settings: TrackSettingsSnapshot;
}

export interface TrackSettingsSnapshot {
  readonly width?: number;
  readonly height?: number;
  readonly frameRate?: number;
  readonly aspectRatio?: number;
  readonly facingMode?: string;
  readonly resizeMode?: string;
}

export interface StatsSnapshot {
  readonly count: number;
  readonly mean: number;
  readonly stdDev: number;
  readonly min: number;
  readonly max: number;
}

export interface FrameTimingSignals {
  readonly source: FrameSource;
  /** `mediaTime`: intervals between camera frame timestamps; `callback`: between frame callbacks. */
  readonly basis: 'mediaTime' | 'callback';
  /** Frame callbacks received. */
  readonly callbacks: number;
  /** Frames the browser presented over the same period (requestVideoFrameCallback only). */
  readonly presentedFrames: number | null;
  readonly durationMs: number;
  readonly intervalMs: StatsSnapshot | null;
}

export interface AnalysisSignals {
  readonly engine: EngineKind;
  readonly delegate: Delegate;
  /** Why the analysis ran on the main thread, when it did. */
  readonly fallbackReason: string | null;
  readonly framesAnalysed: number;
  readonly analysisMs: StatsSnapshot | null;
}

/** Copies the safe, descriptive track settings. */
export function snapshotTrackSettings(settings: MediaTrackSettings): TrackSettingsSnapshot {
  const raw = settings as Readonly<Record<string, unknown>>;
  const snapshot: { -readonly [K in keyof TrackSettingsSnapshot]: TrackSettingsSnapshot[K] } = {};
  const number = (key: string): number | undefined => {
    const value = raw[key];
    return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
  };
  const text = (key: string): string | undefined => {
    const value = raw[key];
    return typeof value === 'string' && value !== '' ? value : undefined;
  };
  const width = number('width');
  const height = number('height');
  const frameRate = number('frameRate');
  const aspectRatio = number('aspectRatio');
  const facingMode = text('facingMode');
  const resizeMode = text('resizeMode');
  if (width !== undefined) snapshot.width = width;
  if (height !== undefined) snapshot.height = height;
  if (frameRate !== undefined) snapshot.frameRate = frameRate;
  if (aspectRatio !== undefined) snapshot.aspectRatio = aspectRatio;
  if (facingMode !== undefined) snapshot.facingMode = facingMode;
  if (resizeMode !== undefined) snapshot.resizeMode = resizeMode;
  return snapshot;
}

/** Online mean / standard deviation / range (Welford). */
export class RunningStats {
  private n = 0;
  private mean = 0;
  private m2 = 0;
  private min = Infinity;
  private max = -Infinity;

  push(value: number): void {
    if (!Number.isFinite(value)) return;
    this.n += 1;
    const delta = value - this.mean;
    this.mean += delta / this.n;
    this.m2 += delta * (value - this.mean);
    if (value < this.min) this.min = value;
    if (value > this.max) this.max = value;
  }

  get count(): number {
    return this.n;
  }

  snapshot(): StatsSnapshot | null {
    if (this.n === 0) return null;
    return {
      count: this.n,
      mean: this.mean,
      stdDev: this.n > 1 ? Math.sqrt(this.m2 / (this.n - 1)) : 0,
      min: this.min,
      max: this.max,
    };
  }

  reset(): void {
    this.n = 0;
    this.mean = 0;
    this.m2 = 0;
    this.min = Infinity;
    this.max = -Infinity;
  }
}

/** Frame metadata the collector reads (a subset of VideoFrameCallbackMetadata). */
export interface FrameMetadataLike {
  readonly mediaTime?: number;
  readonly presentedFrames?: number;
}

/** Frame-interval statistics, from camera timestamps where the browser exposes them. */
export class FrameTimingCollector {
  private readonly intervals = new RunningStats();
  private callbacks = 0;
  private firstNow: number | null = null;
  private lastNow: number | null = null;
  private lastMediaTime: number | null = null;
  private firstPresented: number | null = null;
  private lastPresented: number | null = null;
  private usesMediaTime = true;

  constructor(readonly source: FrameSource) {}

  record(now: number, metadata: FrameMetadataLike | null): void {
    this.callbacks += 1;
    this.firstNow ??= now;
    const mediaTime = metadata?.mediaTime;
    if (this.usesMediaTime && typeof mediaTime === 'number' && mediaTime >= 0) {
      if (this.lastMediaTime !== null) {
        const interval = (mediaTime - this.lastMediaTime) * 1000;
        if (interval > 0) this.intervals.push(interval);
      }
      this.lastMediaTime = mediaTime;
    } else {
      if (this.usesMediaTime && this.callbacks > 1) this.intervals.reset();
      this.usesMediaTime = false;
      if (this.lastNow !== null) this.intervals.push(now - this.lastNow);
    }
    this.lastNow = now;
    const presented = metadata?.presentedFrames;
    if (typeof presented === 'number') {
      this.firstPresented ??= presented;
      this.lastPresented = presented;
    }
  }

  snapshot(): FrameTimingSignals {
    return {
      source: this.source,
      basis: this.usesMediaTime && this.lastMediaTime !== null ? 'mediaTime' : 'callback',
      callbacks: this.callbacks,
      presentedFrames:
        this.firstPresented !== null && this.lastPresented !== null ? this.lastPresented - this.firstPresented + 1 : null,
      durationMs: this.firstNow !== null && this.lastNow !== null ? this.lastNow - this.firstNow : 0,
      intervalMs: this.intervals.snapshot(),
    };
  }
}
