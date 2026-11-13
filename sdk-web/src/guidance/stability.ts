import { distance, type Point } from '../measure/geometry';
import type { StabilityOptions } from '../thresholds';

/** What the stability tracker needs from one analysed frame. */
export interface StabilitySample {
  /** Frame time in milliseconds (monotonic). */
  readonly timestamp: number;
  /** True when no blocking guidance issue remains on this frame. */
  readonly passed: boolean;
  /** Reference point that should stay still (the eye midpoint), in pixels. */
  readonly position: Point | null;
  /** Reference size (the inter-eye distance), in pixels. Movement is measured in units of it. */
  readonly scale: number | null;
  /** Angles that should stay still (yaw, pitch, roll), in degrees. */
  readonly angles?: readonly (number | null)[];
  /** Ranking value for best-frame selection; higher is sharper. Null means "not a candidate". */
  readonly sharpness: number | null;
}

export interface StabilityState {
  /** The current frame passed and continues a still run. */
  readonly stable: boolean;
  /** The current frame passed but moved too much since the previous one; the run restarted. */
  readonly moving: boolean;
  /** Duration of the current still run, in ms. */
  readonly stableMs: number;
  /** Frames in the current still run. */
  readonly frames: number;
  /** The run is long enough to capture. */
  readonly ready: boolean;
  /** Progress towards `ready`, 0-1. */
  readonly progress: number;
}

export interface Candidate<T> {
  readonly sample: StabilitySample;
  readonly payload: T;
}

/** Time deltas are floored to one 60 Hz frame so near-simultaneous frames do not inflate speeds. */
const MIN_FRAME_DELTA_MS = 1000 / 60;

const IDLE: StabilityState = { stable: false, moving: false, stableMs: 0, frames: 0, ready: false, progress: 0 };

/**
 * Tracks runs of consecutive frames that pass every blocking check without moving, and keeps the
 * sharpest frame of the last `windowMs` as the capture candidate.
 *
 * Candidates are kept in a monotonic queue (sliding-window maximum): a frame that is older AND no
 * sharper than a newer one can never be chosen, so it is released at once. That keeps memory
 * bounded even when payloads are full-resolution frames. `retain` is called when the tracker keeps
 * a payload and `release` exactly once when it drops it.
 */
export class StabilityTracker<T> {
  private run: { start: number; frames: number; last: StabilitySample } | null = null;
  private readonly queue: Candidate<T>[] = [];

  constructor(
    private readonly options: StabilityOptions,
    private readonly retain: (payload: T) => void = () => undefined,
    private readonly release: (payload: T) => void = () => undefined,
  ) {}

  push(sample: StabilitySample, payload: T): StabilityState {
    if (!sample.passed) {
      this.reset();
      return IDLE;
    }
    let moving = false;
    const run = this.run;
    if (run !== null) {
      const gap = sample.timestamp - run.last.timestamp;
      if (gap <= 0 || gap > this.options.maxFrameGapMs) {
        this.reset();
      } else if (this.movedTooMuch(run.last, sample, Math.max(gap, MIN_FRAME_DELTA_MS))) {
        moving = true;
        this.reset();
      }
    }
    const current = this.run ?? { start: sample.timestamp, frames: 0, last: sample };
    current.frames += 1;
    current.last = sample;
    this.run = current;

    if (sample.sharpness !== null) {
      while (this.queue.length > 0) {
        const back = this.queue[this.queue.length - 1];
        if (back === undefined || (back.sample.sharpness ?? -Infinity) > sample.sharpness) break;
        this.queue.pop();
        this.release(back.payload);
      }
      this.retain(payload);
      this.queue.push({ sample, payload });
    }
    const horizon = sample.timestamp - this.options.windowMs;
    while (this.queue.length > 0 && (this.queue[0]?.sample.timestamp ?? Infinity) < horizon) {
      const front = this.queue.shift();
      if (front) this.release(front.payload);
    }

    const stableMs = sample.timestamp - current.start;
    const ready = stableMs >= this.options.windowMs && current.frames >= this.options.minFrames;
    const timeProgress = this.options.windowMs > 0 ? stableMs / this.options.windowMs : 1;
    const frameProgress = current.frames / this.options.minFrames;
    return {
      stable: !moving,
      moving,
      stableMs,
      frames: current.frames,
      ready,
      progress: Math.min(1, timeProgress, frameProgress),
    };
  }

  /** The sharpest frame of the current window, if any. */
  best(): Candidate<T> | null {
    return this.queue[0] ?? null;
  }

  /** Drops the current run and releases every kept payload. */
  reset(): void {
    this.run = null;
    for (const candidate of this.queue.splice(0)) this.release(candidate.payload);
  }

  private movedTooMuch(previous: StabilitySample, current: StabilitySample, deltaMs: number): boolean {
    const seconds = deltaMs / 1000;
    if (
      previous.position !== null &&
      current.position !== null &&
      previous.scale !== null &&
      current.scale !== null &&
      current.scale > 0
    ) {
      const shift = distance(previous.position, current.position);
      const resize = Math.abs(current.scale - previous.scale);
      const movement = Math.max(shift, resize) / current.scale / seconds;
      if (movement > this.options.maxMovementPerSecond) return true;
    }
    const before = previous.angles ?? [];
    const after = current.angles ?? [];
    for (let i = 0; i < Math.min(before.length, after.length); i++) {
      const a = before[i];
      const b = after[i];
      if (a === null || a === undefined || b === null || b === undefined) continue;
      if (Math.abs(b - a) / seconds > this.options.maxRotationPerSecond) return true;
    }
    return false;
  }
}
