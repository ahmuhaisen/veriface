import { FaceCaptureError, toFaceCaptureError } from '../errors';
import type { Rect } from '../measure/geometry';
import { SDK_VERSION } from '../version';
import type { Analyzer, AnalyzerConfig, FrameAnalysis, WorkerRequest, WorkerResponse } from './types';

export type EngineKind = 'worker' | 'main-thread';

/** Runs frame analysis, in a Web Worker when possible, otherwise on the main thread. */
export interface Engine {
  readonly kind: EngineKind;
  /** Why the worker was not used, when it was not. */
  readonly fallbackReason: string | null;
  /**
   * Analyses a frame. The engine takes ownership of `frame` and hands it back with the result; on
   * failure it closes the frame itself.
   */
  analyze(frame: ImageBitmap, timestamp: number, guide: Rect | null): Promise<AnalysedFrame>;
  dispose(): void;
}

export interface AnalysedFrame {
  readonly analysis: FrameAnalysis;
  readonly frame: ImageBitmap;
}

export interface EngineOptions {
  readonly config: AnalyzerConfig;
  /** Same-origin worker script URL, or null to stay on the main thread. */
  readonly workerUrl: string | null;
  readonly allowedOrigins: readonly string[];
  /** How long the worker may take to load the Wasm and the model. */
  readonly initTimeoutMs: number;
}

export function workerSupported(): boolean {
  return (
    typeof Worker !== 'undefined' && typeof OffscreenCanvas !== 'undefined' && typeof createImageBitmap === 'function'
  );
}

/**
 * Creates the engine. A worker that fails to start (no OffscreenCanvas WebGL, a blocked or missing
 * worker script, ...) falls back to the main thread; a stale worker script is reported instead,
 * because the other self-hosted assets are then probably stale too.
 */
export async function createEngine(options: EngineOptions): Promise<Engine> {
  let fallbackReason: string;
  if (options.workerUrl === null) {
    fallbackReason = 'worker disabled by configuration';
  } else if (!workerSupported()) {
    fallbackReason = 'Worker, OffscreenCanvas or createImageBitmap is unavailable';
  } else {
    try {
      return await WorkerEngine.create(options.workerUrl, options);
    } catch (error) {
      if (error instanceof FaceCaptureError && error.code === 'asset-version-mismatch') throw error;
      fallbackReason = error instanceof Error ? error.message : String(error);
    }
  }
  const { createAnalyzer } = await import('./analyzer');
  return new MainThreadEngine(await createAnalyzer(options.config), fallbackReason);
}

class MainThreadEngine implements Engine {
  readonly kind = 'main-thread';

  constructor(
    private readonly analyzer: Analyzer,
    readonly fallbackReason: string,
  ) {}

  async analyze(frame: ImageBitmap, timestamp: number, guide: Rect | null): Promise<AnalysedFrame> {
    try {
      return { analysis: await this.analyzer.analyze(frame, timestamp, guide), frame };
    } catch (error) {
      frame.close();
      throw toFaceCaptureError(error);
    }
  }

  dispose(): void {
    this.analyzer.close();
  }
}

interface Pending {
  resolve(response: WorkerResponse): void;
  reject(error: Error): void;
}

class WorkerEngine implements Engine {
  readonly kind = 'worker';
  readonly fallbackReason = null;
  private readonly pending = new Map<number, Pending>();
  private nextId = 1;
  private failure: Error | null = null;
  private disposed = false;

  private constructor(private readonly worker: Worker) {
    worker.addEventListener('message', (event: MessageEvent<WorkerResponse>) => {
      this.onMessage(event.data);
    });
    worker.addEventListener('error', (event: ErrorEvent) => {
      event.preventDefault();
      this.fail(new FaceCaptureError('asset-load-failed', `Analysis worker error: ${event.message || 'the script failed to load'}`));
    });
    worker.addEventListener('messageerror', () => {
      this.fail(new FaceCaptureError('internal', 'Analysis worker sent an unreadable message'));
    });
  }

  static async create(workerUrl: string, options: EngineOptions): Promise<WorkerEngine> {
    const engine = new WorkerEngine(new Worker(workerUrl, { name: 'veriface-capture' }));
    try {
      const id = engine.nextId++;
      const response = await withTimeout(
        engine.request({ type: 'init', id, config: options.config, allowedOrigins: options.allowedOrigins }, id),
        options.initTimeoutMs,
        'The analysis worker did not start in time',
      );
      if (response.type === 'error') throw new FaceCaptureError(response.code, response.message);
      if (response.type !== 'ready') throw new FaceCaptureError('internal', 'Unexpected worker response');
      if (response.version !== SDK_VERSION) {
        throw new FaceCaptureError(
          'asset-version-mismatch',
          `The self-hosted face-capture.worker.js is version ${response.version}, but the SDK is ${SDK_VERSION}. Copy the assets of this SDK version again.`,
        );
      }
      return engine;
    } catch (error) {
      engine.dispose();
      throw error;
    }
  }

  async analyze(frame: ImageBitmap, timestamp: number, guide: Rect | null): Promise<AnalysedFrame> {
    const id = this.nextId++;
    let response: WorkerResponse;
    try {
      response = await this.request({ type: 'analyze', id, frame, timestamp, guide }, id, [frame]);
    } catch (error) {
      frame.close(); // no-op when it was already transferred
      throw toFaceCaptureError(error);
    }
    if (response.type === 'result') return { analysis: response.analysis, frame: response.frame };
    if (response.type === 'error') {
      response.frame?.close();
      throw new FaceCaptureError(response.code, response.message);
    }
    throw new FaceCaptureError('internal', 'Unexpected worker response');
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    try {
      const message: WorkerRequest = { type: 'dispose' };
      this.worker.postMessage(message);
    } catch {
      // The worker is already gone.
    }
    this.worker.terminate();
    this.fail(new FaceCaptureError('disposed', 'The analysis engine was disposed'));
  }

  private request(message: WorkerRequest, id: number, transfer: Transferable[] = []): Promise<WorkerResponse> {
    if (this.failure) return Promise.reject(this.failure);
    return new Promise<WorkerResponse>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      try {
        this.worker.postMessage(message, transfer);
      } catch (error) {
        this.pending.delete(id);
        reject(toFaceCaptureError(error));
      }
    });
  }

  private onMessage(response: WorkerResponse): void {
    const pending = this.pending.get(response.id);
    if (!pending) {
      if (response.type === 'result' || response.type === 'error') response.frame?.close();
      return;
    }
    this.pending.delete(response.id);
    pending.resolve(response);
  }

  private fail(error: Error): void {
    this.failure ??= error;
    for (const pending of this.pending.values()) pending.reject(error);
    this.pending.clear();
  }
}

function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      reject(new FaceCaptureError('asset-load-failed', message));
    }, ms);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error: unknown) => {
        clearTimeout(timer);
        reject(error instanceof Error ? error : new Error(String(error)));
      },
    );
  });
}
