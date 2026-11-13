import type { Affine, CropLayout, PixelBuffer } from '../measure/pixels';

/** The subset of the 2D context API the analyzers use; both canvas kinds provide it. */
export interface Context2D {
  imageSmoothingEnabled: boolean;
  imageSmoothingQuality: ImageSmoothingQuality;
  setTransform(a: number, b: number, c: number, d: number, e: number, f: number): void;
  clearRect(x: number, y: number, width: number, height: number): void;
  drawImage(image: CanvasImageSource, dx: number, dy: number): void;
  drawImage(
    image: CanvasImageSource,
    sx: number,
    sy: number,
    sw: number,
    sh: number,
    dx: number,
    dy: number,
    dw: number,
    dh: number,
  ): void;
  getImageData(sx: number, sy: number, sw: number, sh: number): ImageData;
}

export interface Surface {
  readonly canvas: OffscreenCanvas | HTMLCanvasElement;
  readonly context: Context2D;
}

/** A 2D canvas that works both in a worker (OffscreenCanvas) and on the main thread. */
export function createSurface(width: number, height: number): Surface {
  let canvas: OffscreenCanvas | HTMLCanvasElement;
  if (typeof OffscreenCanvas !== 'undefined') {
    canvas = new OffscreenCanvas(width, height);
  } else if (typeof document !== 'undefined') {
    canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
  } else {
    throw new Error('No canvas implementation is available');
  }
  const context = canvas.getContext('2d', { willReadFrequently: true }) as Context2D | null;
  if (context === null) throw new Error('2D canvas context is unavailable');
  return { canvas, context };
}

export function resizeSurface(surface: Surface, width: number, height: number): void {
  if (surface.canvas.width !== width) surface.canvas.width = width;
  if (surface.canvas.height !== height) surface.canvas.height = height;
}

/**
 * Draws the source through `transform` (see `cropTransform`) into a `layout`-sized crop: the face
 * de-rotated, with the eye midpoint at (originX, originY) and the inter-eye distance scaled to
 * `layout.interEyeDistance`. Pixels outside the source image stay transparent, so the statistics
 * can ignore them.
 */
export function sampleAlignedCrop(
  surface: Surface,
  source: CanvasImageSource,
  transform: Affine,
  layout: CropLayout,
): PixelBuffer {
  resizeSurface(surface, layout.width, layout.height);
  const ctx = surface.context;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, layout.width, layout.height);
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = 'high';
  ctx.setTransform(transform.a, transform.b, transform.c, transform.d, transform.e, transform.f);
  ctx.drawImage(source, 0, 0);
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  return ctx.getImageData(0, 0, layout.width, layout.height);
}
