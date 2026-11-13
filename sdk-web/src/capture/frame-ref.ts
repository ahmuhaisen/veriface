/** Anything with a close() method, typically an ImageBitmap. */
export interface Closable {
  close(): void;
}

/**
 * Reference-counted frame. A full-resolution frame can be held at the same time by the capture
 * candidate queue, the burst ring and the "latest frame" slot; it is closed when the last holder
 * releases it, so GPU and CPU memory are freed promptly.
 */
export class FrameRef<B extends Closable = ImageBitmap> {
  private count = 1;

  constructor(
    readonly bitmap: B,
    readonly timestamp: number,
  ) {}

  get released(): boolean {
    return this.count === 0;
  }

  retain(): this {
    if (this.count === 0) throw new Error('FrameRef used after release');
    this.count += 1;
    return this;
  }

  release(): void {
    if (this.count === 0) return;
    this.count -= 1;
    if (this.count === 0) this.bitmap.close();
  }
}
