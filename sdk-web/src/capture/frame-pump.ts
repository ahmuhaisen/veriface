export type FrameSource = 'requestVideoFrameCallback' | 'requestAnimationFrame';

export type FrameCallback = (now: number, metadata: VideoFrameCallbackMetadata | null) => void;

/**
 * Calls back once per new video frame: `requestVideoFrameCallback` where available, otherwise
 * `requestAnimationFrame` plus a check that the video time moved.
 */
export class FramePump {
  readonly source: FrameSource;
  private running = false;
  private handle: number | null = null;
  private lastTime = -1;

  constructor(
    private readonly video: HTMLVideoElement,
    private readonly onFrame: FrameCallback,
  ) {
    this.source = typeof video.requestVideoFrameCallback === 'function' ? 'requestVideoFrameCallback' : 'requestAnimationFrame';
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    this.schedule();
  }

  stop(): void {
    this.running = false;
    if (this.handle === null) return;
    if (this.source === 'requestVideoFrameCallback') this.video.cancelVideoFrameCallback(this.handle);
    else cancelAnimationFrame(this.handle);
    this.handle = null;
  }

  private schedule(): void {
    if (this.source === 'requestVideoFrameCallback') {
      this.handle = this.video.requestVideoFrameCallback((now, metadata) => {
        if (!this.running) return;
        this.schedule();
        this.onFrame(now, metadata);
      });
    } else {
      this.handle = requestAnimationFrame((now) => {
        if (!this.running) return;
        this.schedule();
        const time = this.video.currentTime;
        if (time === this.lastTime) return;
        this.lastTime = time;
        this.onFrame(now, null);
      });
    }
  }
}
