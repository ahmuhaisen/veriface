import type { ResolvedAssets } from '../analysis/assets';
import { createEngine, type Engine, type EngineKind } from '../analysis/engine';
import type { CaptureMode, Delegate, FrameAnalysis } from '../analysis/types';
import { CATALOGUE_VERSION } from '../contract';
import { FaceCaptureError, toCameraError, toFaceCaptureError } from '../errors';
import { HintSmoother } from '../guidance/hint';
import { StabilityTracker } from '../guidance/stability';
import type { FrameGuidance, GuidanceCode } from '../guidance/types';
import type { MessageKey } from '../i18n';
import type { Point, Rect } from '../measure/geometry';
import { resolveStability, type ProfileLike, type StabilityOptions } from '../thresholds';
import { SDK_VERSION } from '../version';
import { assertCaptureSupport, attachStream, buildVideoConstraints, openCamera, stopStream } from './camera';
import { Emitter } from './emitter';
import { DEFAULT_JPEG_QUALITY, encodeJpeg } from './encode';
import { FramePump } from './frame-pump';
import { FrameRef } from './frame-ref';
import {
  FrameTimingCollector,
  RunningStats,
  snapshotTrackSettings,
  type CameraSignals,
  type CaptureSignals,
} from './signals';
import type {
  BurstFrame,
  CaptureController,
  CaptureEventMap,
  CaptureResult,
  CaptureStatus,
  CommonCaptureOptions,
  GuidanceState,
} from './types';

/** One analysed frame turned into guidance plus the values the stability tracker needs. */
export interface EvaluatedFrame<C extends GuidanceCode, M> {
  readonly guidance: FrameGuidance<C>;
  readonly measurements: M;
  readonly position: Point | null;
  readonly scale: number | null;
  readonly angles: readonly (number | null)[];
  readonly sharpness: number | null;
}

/** What differs between selfie and document capture. */
export interface ModeStrategy<C extends GuidanceCode, M> {
  readonly mode: CaptureMode;
  /** Codes in priority order (used to break ties when smoothing hints). */
  readonly codes: readonly C[];
  readonly holdStillCode: C;
  readonly readyCode: C;
  readonly modelUrl: string;
  readonly modelSha256: string | null;
  readonly defaultStability: StabilityOptions;
  readonly profile: ProfileLike | undefined;
  evaluate(analysis: FrameAnalysis): EvaluatedFrame<C, M>;
  messageKey(code: C): MessageKey;
  /** Region of interest in frame pixels (the document card guide), or null. */
  guide(frameWidth: number, frameHeight: number): Rect | null;
}

export interface SessionOptions {
  readonly autoCapture: boolean;
  readonly burstCount: number;
  readonly jpegQuality: number;
  readonly maxAnalysisFps: number;
  readonly hintWindowMs: number;
  readonly keepCameraOnCapture: boolean;
  readonly mirrorPreview: boolean | null;
  readonly delegate: Delegate;
  readonly initTimeoutMs: number;
  readonly stability: StabilityOptions;
}

export const MAX_BURST_FRAMES = 10;

/** Validates and fills in the options shared by both capture modes. */
export function resolveSessionOptions(options: CommonCaptureOptions, defaultStability: StabilityOptions): SessionOptions {
  const invalid = (message: string): never => {
    throw new FaceCaptureError('invalid-options', message);
  };
  const burstCount = options.burstCount ?? 0;
  if (!Number.isInteger(burstCount) || burstCount < 0 || burstCount > MAX_BURST_FRAMES) {
    invalid(`burstCount must be an integer from 0 to ${MAX_BURST_FRAMES}`);
  }
  const jpegQuality = options.jpegQuality ?? DEFAULT_JPEG_QUALITY;
  if (!(jpegQuality > 0 && jpegQuality <= 1)) invalid('jpegQuality must be within (0, 1]');
  const maxAnalysisFps = options.maxAnalysisFps ?? 15;
  if (!(maxAnalysisFps > 0 && maxAnalysisFps <= 60)) invalid('maxAnalysisFps must be within (0, 60]');
  const hintWindowMs = options.hintWindowMs ?? 400;
  if (!(hintWindowMs >= 0 && Number.isFinite(hintWindowMs))) invalid('hintWindowMs must be a non-negative number');
  const initTimeoutMs = options.initTimeoutMs ?? 60_000;
  if (!(initTimeoutMs > 0 && Number.isFinite(initTimeoutMs))) invalid('initTimeoutMs must be a positive number');
  const delegate = parseDelegate(options.delegate);
  if (typeof HTMLVideoElement !== 'undefined' && !(options.video instanceof HTMLVideoElement)) {
    invalid('video must be an HTMLVideoElement');
  }
  return {
    autoCapture: options.autoCapture ?? true,
    burstCount,
    jpegQuality,
    maxAnalysisFps,
    hintWindowMs,
    keepCameraOnCapture: options.keepCameraOnCapture ?? false,
    mirrorPreview: options.mirrorPreview ?? null,
    delegate,
    initTimeoutMs,
    stability: resolveStability(defaultStability, options.stability),
  };
}

/** Options may come from untyped JavaScript or JSON, so the delegate is validated at runtime. */
function parseDelegate(value: unknown): Delegate {
  if (value === undefined) return 'CPU';
  if (value === 'CPU' || value === 'GPU') return value;
  throw new FaceCaptureError('invalid-options', 'delegate must be "CPU" or "GPU"');
}

interface AnalysedFrame<C extends GuidanceCode, M> {
  readonly ref: FrameRef;
  state: GuidanceState<C, M> | null;
}

const NO_TIMING = new FrameTimingCollector('requestAnimationFrame').snapshot();

/** The capture loop shared by selfie and document capture. */
export class CaptureSession<C extends GuidanceCode, M> implements CaptureController<C, M> {
  readonly mode: CaptureMode;
  readonly video: HTMLVideoElement;
  private currentStatus: CaptureStatus = 'idle';
  private currentGuidance: GuidanceState<C, M> | null = null;
  private currentGuide: Rect | null = null;
  private isMirrored = false;
  private analysisEngine: Engine | null = null;
  private stream: MediaStream | null = null;
  private pump: FramePump | null = null;
  private timing: FrameTimingCollector | null = null;
  private readonly analysisStats = new RunningStats();
  private readonly tracker: StabilityTracker<AnalysedFrame<C, M>>;
  private readonly smoother: HintSmoother<C>;
  private readonly emitter = new Emitter<CaptureEventMap<C, M>>();
  private burst: FrameRef[] = [];
  private latest: AnalysedFrame<C, M> | null = null;
  private busy = false;
  private lastAnalysisAt = -Infinity;
  /** Bumped by stop, dispose and failures, so work started earlier knows it is stale. */
  private generation = 0;
  private startPromise: Promise<void> | null = null;
  private capturing: Promise<CaptureResult<C, M>> | null = null;
  private cameraInfo: CameraSignals = { label: '', settings: {} };
  private frameWaiters: { resolve(): void; reject(error: Error): void }[] = [];
  private readonly constraints: MediaTrackConstraints;
  private readonly minFrameIntervalMs: number;

  constructor(
    private readonly strategy: ModeStrategy<C, M>,
    options: CommonCaptureOptions,
    private readonly settings: SessionOptions,
    private readonly assets: ResolvedAssets,
  ) {
    this.mode = strategy.mode;
    this.video = options.video;
    this.constraints = buildVideoConstraints(strategy.mode, options.deviceId, options.videoConstraints);
    this.minFrameIntervalMs = 1000 / settings.maxAnalysisFps;
    this.tracker = new StabilityTracker<AnalysedFrame<C, M>>(
      settings.stability,
      (frame) => frame.ref.retain(),
      (frame) => {
        frame.ref.release();
      },
    );
    const rank = new Map(strategy.codes.map((code, index) => [code, index]));
    this.smoother = new HintSmoother<C>(
      settings.hintWindowMs,
      (code) => rank.get(code) ?? Number.MAX_SAFE_INTEGER,
      new Set([strategy.readyCode]),
    );
  }

  get status(): CaptureStatus {
    return this.currentStatus;
  }

  get guidance(): GuidanceState<C, M> | null {
    return this.currentGuidance;
  }

  get engine(): EngineKind | null {
    return this.analysisEngine?.kind ?? null;
  }

  get mirrored(): boolean {
    return this.isMirrored;
  }

  get guide(): Rect | null {
    return this.currentGuide;
  }

  on<K extends keyof CaptureEventMap<C, M>>(type: K, listener: (value: CaptureEventMap<C, M>[K]) => void): () => void {
    return this.emitter.on(type, listener);
  }

  start(): Promise<void> {
    if (this.currentStatus === 'disposed') {
      return Promise.reject(new FaceCaptureError('disposed', 'This capture was disposed'));
    }
    if (this.currentStatus === 'running') return Promise.resolve();
    if (this.capturing !== null) {
      // Let the capture in progress finish, then start again (for example a quick "retake").
      return this.capturing.then(
        () => this.start(),
        () => this.start(),
      );
    }
    if (this.currentStatus === 'captured' && this.stream?.active === true && this.pump !== null) {
      // keepCameraOnCapture: resume on the open camera.
      this.resetGuidance();
      this.setStatus('running');
      this.pump.start();
      return Promise.resolve();
    }
    this.startPromise ??= this.doStart().finally(() => {
      this.startPromise = null;
    });
    return this.startPromise;
  }

  capture(): Promise<CaptureResult<C, M>> {
    if (this.capturing) return this.capturing;
    if (this.currentStatus !== 'running') {
      return Promise.reject(new FaceCaptureError('not-running', 'Start the capture before capturing'));
    }
    const ready = this.latest === null ? this.waitForFrame(5000) : Promise.resolve();
    return ready.then(() => this.captureNow('manual'));
  }

  stop(): void {
    if (this.currentStatus === 'disposed') return;
    this.generation += 1;
    this.releaseCamera();
    this.releaseFrames();
    this.rejectFrameWaiters(new FaceCaptureError('not-running', 'The capture was stopped'));
    this.setStatus('stopped');
  }

  dispose(): void {
    if (this.currentStatus === 'disposed') return;
    this.generation += 1;
    this.releaseCamera();
    this.releaseFrames();
    this.rejectFrameWaiters(new FaceCaptureError('disposed', 'The capture was disposed'));
    this.analysisEngine?.dispose();
    this.analysisEngine = null;
    this.setStatus('disposed');
    this.emitter.clear();
  }

  private async doStart(): Promise<void> {
    this.generation += 1;
    const generation = this.generation;
    this.setStatus('starting');
    let stream: MediaStream | null = null;
    try {
      assertCaptureSupport();
      const engine = this.analysisEngine
        ? Promise.resolve(this.analysisEngine)
        : createEngine({
            config: {
              mode: this.mode,
              wasmBaseUrl: this.assets.wasmBaseUrl,
              modelUrl: this.strategy.modelUrl,
              modelSha256: this.strategy.modelSha256,
              delegate: this.settings.delegate,
            },
            workerUrl: this.assets.workerUrl,
            allowedOrigins: this.assets.origins,
            initTimeoutMs: this.settings.initTimeoutMs,
          });
      // Load the model while the browser asks for camera permission.
      const [engineResult, streamResult] = await Promise.allSettled([engine, openCamera(this.constraints)]);
      if (streamResult.status === 'fulfilled') stream = streamResult.value;
      if (engineResult.status === 'fulfilled') this.analysisEngine = engineResult.value;
      if (generation !== this.generation) {
        stopStream(stream);
        if (this.currentStatus === 'disposed') {
          this.analysisEngine?.dispose();
          this.analysisEngine = null;
        }
        return;
      }
      if (streamResult.status === 'rejected') throw toCameraError(streamResult.reason);
      if (engineResult.status === 'rejected') throw toFaceCaptureError(engineResult.reason, 'asset-load-failed');
      if (stream === null) throw new FaceCaptureError('internal', 'No camera stream');
      // An already-ended track never fires 'ended', so it would leave guidance waiting forever.
      const liveTrack = stream.getVideoTracks()[0];
      if (liveTrack?.readyState !== 'live') {
        stopStream(stream);
        throw new FaceCaptureError('camera-unavailable', 'The camera stream has no live video track');
      }

      this.stream = stream;
      await attachStream(this.video, stream);
      if (generation !== this.generation) return;

      const track = stream.getVideoTracks()[0];
      track?.addEventListener('ended', this.onTrackEnded);
      const settings = track?.getSettings() ?? {};
      this.cameraInfo = { label: track?.label ?? '', settings: snapshotTrackSettings(settings) };
      this.isMirrored =
        this.settings.mirrorPreview ?? (this.mode === 'selfie' && settings.facingMode !== 'environment');
      this.video.style.transform = this.isMirrored ? 'scaleX(-1)' : '';

      this.resetGuidance();
      this.pump = new FramePump(this.video, this.onFrame);
      this.timing = new FrameTimingCollector(this.pump.source);
      this.setStatus('running');
      this.pump.start();
    } catch (error) {
      if (generation !== this.generation) {
        stopStream(stream);
        return;
      }
      if (this.stream !== stream) stopStream(stream);
      const failure = toFaceCaptureError(error);
      this.fail(failure);
      throw failure;
    }
  }

  private readonly onFrame = (now: number, metadata: VideoFrameCallbackMetadata | null): void => {
    this.timing?.record(now, metadata);
    if (this.currentStatus !== 'running' || this.busy) return;
    if (now - this.lastAnalysisAt < this.minFrameIntervalMs) return;
    this.lastAnalysisAt = now;
    this.busy = true;
    const generation = this.generation;
    void this.processFrame(now, generation)
      .catch((error: unknown) => {
        // Unexpected failures (for example in a strategy) end the capture with an 'error' event.
        if (generation === this.generation) this.fail(toFaceCaptureError(error));
      })
      .finally(() => {
        this.busy = false;
      });
  };

  private readonly onTrackEnded = (): void => {
    if (this.currentStatus === 'running' || this.currentStatus === 'capturing') {
      this.fail(new FaceCaptureError('camera-unavailable', 'The camera stopped delivering video'));
    }
  };

  private async processFrame(timestamp: number, generation: number): Promise<void> {
    const engine = this.analysisEngine;
    if (engine === null) return;
    let bitmap: ImageBitmap;
    try {
      bitmap = await createImageBitmap(this.video);
    } catch {
      return; // no decodable frame yet
    }
    if (!this.isCurrent(generation)) {
      bitmap.close();
      return;
    }
    const guide = this.strategy.guide(bitmap.width, bitmap.height);
    this.currentGuide = guide;
    let analysed: { analysis: FrameAnalysis; frame: ImageBitmap };
    try {
      analysed = await engine.analyze(bitmap, timestamp, guide);
    } catch (error) {
      if (generation === this.generation) {
        engine.dispose();
        if (this.analysisEngine === engine) this.analysisEngine = null;
        this.fail(toFaceCaptureError(error));
      }
      return;
    }
    const ref = new FrameRef(analysed.frame, timestamp);
    try {
      if (!this.isCurrent(generation)) return;
      this.analysisStats.push(analysed.analysis.timing.totalMs);
      const evaluated = this.strategy.evaluate(analysed.analysis);
      const frame: AnalysedFrame<C, M> = { ref, state: null };
      const stability = this.tracker.push(
        {
          timestamp,
          passed: evaluated.guidance.passed,
          position: evaluated.position,
          scale: evaluated.scale,
          angles: evaluated.angles,
          sharpness: evaluated.sharpness,
        },
        frame,
      );
      const frameCode =
        evaluated.guidance.issues[0]?.code ?? (stability.ready ? this.strategy.readyCode : this.strategy.holdStillCode);
      const code = this.smoother.update(frameCode, timestamp);
      const state: GuidanceState<C, M> = {
        mode: this.mode,
        code,
        messageKey: this.strategy.messageKey(code),
        frameCode,
        issues: evaluated.guidance.issues,
        advisories: evaluated.guidance.advisories,
        checks: evaluated.guidance.checks,
        passed: evaluated.guidance.passed,
        stability,
        measurements: evaluated.measurements,
        frame: {
          width: analysed.frame.width,
          height: analysed.frame.height,
          timestamp,
          analysisMs: analysed.analysis.timing.totalMs,
        },
      };
      frame.state = state;
      this.keepLatest(frame);
      this.keepBurst(ref);
      this.currentGuidance = state;
      this.emitter.emit('guidance', state);
      this.resolveFrameWaiters();
      // Re-checked: a 'guidance' listener may have stopped the capture.
      if (this.settings.autoCapture && stability.ready && this.isCurrent(generation)) {
        // Failures are reported through the 'error' event by captureNow.
        await this.captureNow('auto').catch(() => undefined);
      }
    } finally {
      ref.release();
    }
  }

  /**
   * Whether work started in `generation` may continue. A method on purpose: the status changes
   * across awaits and listener calls, which type narrowing cannot see.
   */
  private isCurrent(generation: number): boolean {
    return generation === this.generation && this.currentStatus === 'running';
  }

  private captureNow(trigger: 'auto' | 'manual'): Promise<CaptureResult<C, M>> {
    this.capturing ??= this.doCapture(trigger).finally(() => {
      this.capturing = null;
    });
    return this.capturing;
  }

  private async doCapture(trigger: 'auto' | 'manual'): Promise<CaptureResult<C, M>> {
    const best = this.tracker.best()?.payload ?? null;
    const chosen = best ?? this.latest;
    const state = chosen?.state ?? null;
    if (chosen === null || state === null) {
      throw new FaceCaptureError('not-running', 'No analysed frame is available yet');
    }
    const generation = this.generation;
    const latestState = this.currentGuidance;
    const ready = best !== null && latestState !== null && latestState.passed && latestState.stability.ready;
    const stableMs = latestState?.stability.stableMs ?? 0;
    const ref = chosen.ref.retain();
    const burstRefs = this.burst.map((r) => r.retain());
    this.setStatus('capturing');
    this.pump?.stop();
    this.releaseFrames();
    try {
      const width = ref.bitmap.width;
      const height = ref.bitmap.height;
      const blob = await encodeJpeg(ref.bitmap, this.settings.jpegQuality);
      const burst: BurstFrame[] = [];
      for (const frame of burstRefs) {
        burst.push({
          blob: await encodeJpeg(frame.bitmap, this.settings.jpegQuality),
          width: frame.bitmap.width,
          height: frame.bitmap.height,
          offsetMs: frame.timestamp - ref.timestamp,
        });
      }
      if (generation !== this.generation) throw new FaceCaptureError('not-running', 'The capture was stopped');
      const profile = this.strategy.profile;
      const result: CaptureResult<C, M> = {
        mode: this.mode,
        blob,
        mimeType: 'image/jpeg',
        width,
        height,
        capturedAt: new Date().toISOString(),
        guidance: {
          trigger,
          ready,
          issues: state.issues,
          advisories: state.advisories,
          checks: state.checks,
          measurements: state.measurements,
          stableMs,
          profile: profile
            ? { name: profile.name ?? null, version: profile.version ?? null }
            : { name: 'standard', version: '0.1.0' },
          catalogueVersion: CATALOGUE_VERSION,
        },
        signals: this.signals(),
        burst,
      };
      if (!this.settings.keepCameraOnCapture) this.releaseCamera();
      this.setStatus('captured');
      this.emitter.emit('capture', result);
      return result;
    } catch (error) {
      const failure = toFaceCaptureError(error);
      if (generation === this.generation) this.fail(failure);
      throw failure;
    } finally {
      ref.release();
      for (const frame of burstRefs) frame.release();
    }
  }

  private signals(): CaptureSignals {
    return {
      sdkVersion: SDK_VERSION,
      camera: this.cameraInfo,
      frameTiming: this.timing?.snapshot() ?? NO_TIMING,
      analysis: {
        engine: this.analysisEngine?.kind ?? 'main-thread',
        delegate: this.settings.delegate,
        fallbackReason: this.analysisEngine?.fallbackReason ?? null,
        framesAnalysed: this.analysisStats.count,
        analysisMs: this.analysisStats.snapshot(),
      },
    };
  }

  private keepLatest(frame: AnalysedFrame<C, M>): void {
    frame.ref.retain();
    this.latest?.ref.release();
    this.latest = frame;
  }

  private keepBurst(ref: FrameRef): void {
    if (this.settings.burstCount === 0) return;
    this.burst.push(ref.retain());
    while (this.burst.length > this.settings.burstCount) this.burst.shift()?.release();
  }

  private resetGuidance(): void {
    this.releaseFrames();
    this.smoother.reset();
    this.analysisStats.reset();
    this.lastAnalysisAt = -Infinity;
    this.currentGuidance = null;
  }

  private releaseFrames(): void {
    this.tracker.reset();
    for (const ref of this.burst) ref.release();
    this.burst = [];
    this.latest?.ref.release();
    this.latest = null;
  }

  private releaseCamera(): void {
    this.pump?.stop();
    this.pump = null;
    const stream = this.stream;
    this.stream = null;
    stream?.getVideoTracks()[0]?.removeEventListener('ended', this.onTrackEnded);
    stopStream(stream);
    if (stream !== null && this.video.srcObject === stream) this.video.srcObject = null;
  }

  private fail(error: FaceCaptureError): void {
    this.generation += 1;
    this.releaseCamera();
    this.releaseFrames();
    this.rejectFrameWaiters(error);
    this.setStatus('error');
    this.emitter.emit('error', error);
  }

  private setStatus(status: CaptureStatus): void {
    if (this.currentStatus === status) return;
    this.currentStatus = status;
    this.emitter.emit('status', status);
  }

  private waitForFrame(timeoutMs: number): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.frameWaiters = this.frameWaiters.filter((w) => w !== waiter);
        reject(new FaceCaptureError('not-running', 'No camera frame was analysed in time'));
      }, timeoutMs);
      const waiter = {
        resolve: () => {
          clearTimeout(timer);
          resolve();
        },
        reject: (error: Error) => {
          clearTimeout(timer);
          reject(error);
        },
      };
      this.frameWaiters.push(waiter);
    });
  }

  private resolveFrameWaiters(): void {
    for (const waiter of this.frameWaiters.splice(0)) waiter.resolve();
  }

  private rejectFrameWaiters(error: Error): void {
    for (const waiter of this.frameWaiters.splice(0)) waiter.reject(error);
  }
}
