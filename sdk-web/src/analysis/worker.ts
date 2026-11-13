/**
 * Analysis Web Worker. Built as one self-contained classic script (`face-capture.worker.js`) with
 * MediaPipe bundled in; a classic worker lets MediaPipe load its Wasm loader via importScripts().
 * MediaPipe's detect calls are synchronous and block their thread, which is why they run here.
 */
import { FaceCaptureError, toFaceCaptureError } from '../errors';
import { SDK_VERSION } from '../version';
import { createAnalyzer } from './analyzer';
import { installFetchGuard, type FetchScope } from './fetch-guard';
import type { Analyzer, WorkerRequest, WorkerResponse } from './types';

interface WorkerScope extends FetchScope {
  postMessage(message: WorkerResponse, transfer?: Transferable[]): void;
  addEventListener(type: 'message', listener: (event: MessageEvent<WorkerRequest>) => void): void;
  close(): void;
}

const scope = self as unknown as WorkerScope;
let analyzer: Analyzer | null = null;
let queue: Promise<void> = Promise.resolve();

scope.addEventListener('message', (event) => {
  const message = event.data;
  // One message at a time: frames are analysed in order and never concurrently.
  queue = queue.then(() => handle(message));
});

async function handle(message: WorkerRequest): Promise<void> {
  switch (message.type) {
    case 'init': {
      try {
        installFetchGuard(scope, message.allowedOrigins);
        analyzer = await createAnalyzer(message.config);
        scope.postMessage({ type: 'ready', id: message.id, version: SDK_VERSION });
      } catch (error) {
        const e = toFaceCaptureError(error, 'asset-load-failed');
        scope.postMessage({ type: 'error', id: message.id, code: e.code, message: describe(e), frame: null });
      }
      return;
    }
    case 'analyze': {
      const { frame } = message;
      try {
        if (analyzer === null) throw new FaceCaptureError('internal', 'The analysis worker is not initialised');
        const analysis = await analyzer.analyze(frame, message.timestamp, message.guide);
        scope.postMessage({ type: 'result', id: message.id, analysis, frame }, [frame]);
      } catch (error) {
        const e = toFaceCaptureError(error);
        scope.postMessage({ type: 'error', id: message.id, code: e.code, message: describe(e), frame }, [frame]);
      }
      return;
    }
    case 'dispose': {
      analyzer?.close();
      analyzer = null;
      scope.close();
      return;
    }
  }
}

function describe(error: Error): string {
  const cause = error.cause instanceof Error ? `: ${error.cause.message}` : '';
  return `${error.message}${cause}`;
}
