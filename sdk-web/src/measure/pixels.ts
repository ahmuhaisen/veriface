/**
 * Pixel statistics of a face, computed on an "aligned face crop": the face de-rotated so the eye
 * line is horizontal and scaled to a fixed inter-eye distance. That makes the numbers independent of
 * camera resolution and face size, so frames can be compared and thresholds stay stable.
 *
 * Luma and the exposure levels follow checks.yaml `definitions` (luma, underExposureLevel,
 * overExposureLevel); a contract test pins them.
 */
import type { Point } from './geometry';

/** RGBA pixels, row-major, 4 bytes per pixel (the shape of ImageData). */
export interface PixelBuffer {
  readonly data: ArrayLike<number>;
  readonly width: number;
  readonly height: number;
}

/** Placement of the eye midpoint and the scale inside an aligned crop. */
export interface CropLayout {
  readonly width: number;
  readonly height: number;
  /** Where the eye midpoint lands in the crop, in crop pixels. */
  readonly originX: number;
  readonly originY: number;
  /** Inter-eye distance in crop pixels. */
  readonly interEyeDistance: number;
}

/** A rectangle in inter-eye-distance units around the eye midpoint (x right, y down). */
export interface FaceRegion {
  readonly left: number;
  readonly right: number;
  readonly top: number;
  readonly bottom: number;
}

const CROP_IED = 50;

/**
 * The aligned crop: 2.8 inter-eye distances wide and 4.0 high (1.6 above the eye line, 2.4 below),
 * large enough to hold the whole landmark face oval, forehead to chin.
 */
export const FACE_CROP: CropLayout = {
  width: 2.8 * CROP_IED,
  height: 4 * CROP_IED,
  originX: 1.4 * CROP_IED,
  originY: 1.6 * CROP_IED,
  interEyeDistance: CROP_IED,
};

/** A 2-D affine map: x' = a x + c y + e, y' = b x + d y + f (the canvas setTransform order). */
export interface Affine {
  readonly a: number;
  readonly b: number;
  readonly c: number;
  readonly d: number;
  readonly e: number;
  readonly f: number;
}

/**
 * Image pixels → aligned-crop pixels: de-rotate by the eye-line roll around the eye midpoint, scale
 * the inter-eye distance to `layout.interEyeDistance`, and put the eye midpoint at the layout origin.
 */
export function cropTransform(eyeMidpoint: Point, rollDegrees: number, interEyeDistance: number, layout: CropLayout): Affine {
  const scale = layout.interEyeDistance / interEyeDistance;
  const angle = (-rollDegrees * Math.PI) / 180;
  const a = scale * Math.cos(angle);
  const b = scale * Math.sin(angle);
  const c = -b;
  const d = a;
  return {
    a,
    b,
    c,
    d,
    e: layout.originX - (a * eyeMidpoint.x + c * eyeMidpoint.y),
    f: layout.originY - (b * eyeMidpoint.x + d * eyeMidpoint.y),
  };
}

export function applyAffine(t: Affine, p: Point): Point {
  return { x: t.a * p.x + t.c * p.y + t.e, y: t.b * p.x + t.d * p.y + t.f };
}

/**
 * Fallback skin rectangle (cheeks and nose between the eyes and the mouth) for when no landmark mesh
 * is available, as for document portraits found by the face detector.
 */
export const SKIN_REGION: FaceRegion = { left: -0.6, right: 0.6, top: 0.2, bottom: 0.9 };

/** Brows to chin, cheek to cheek: the whole face, used for sharpness and portrait glare. */
export const FACE_REGION: FaceRegion = { left: -0.9, right: 0.9, top: -0.6, bottom: 1.6 };

/** Luma (0-255) at or below this level counts as under-exposed. */
export const UNDER_EXPOSURE_LEVEL = 25;
/** Luma (0-255) at or above this level counts as clipped bright (over-exposed, glare). */
export const OVER_EXPOSURE_LEVEL = 247;
/** Pixels with lower alpha lie outside the source image and are ignored. */
const OPAQUE = 250;
/** Minimum fraction of a region that must lie inside the image for its statistics to count. */
export const MIN_COVERAGE = 0.6;

/**
 * Sharpness score mapping: score = raw / (raw + SHARPNESS_HALF_POINT). PROVISIONAL: chosen by hand,
 * not calibrated. The service computes SHARPNESS with its own method, so the two scores are only
 * loosely comparable; see the README.
 */
export const SHARPNESS_HALF_POINT = 0.25;

/** Luma planes of a crop, plus a validity mask for pixels inside the source image. */
export interface LumaPlane {
  readonly width: number;
  readonly height: number;
  /** Contract luma (OFIQ, linear 0-255 integers): exposure statistics and LUMINANCE_MEAN. */
  readonly luma: Float32Array;
  /** Gamma-encoded grey (BT.601 Y'): used only for the SDK's own sharpness estimator. */
  readonly grey: Float32Array;
  readonly valid: Uint8Array;
}

/** sRGB transfer curve: 8-bit encoded value to linear light, 0-1. */
export function srgbToLinear(value: number): number {
  const c = value / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

const LINEAR = Float64Array.from({ length: 256 }, (_, v) => srgbToLinear(v));

/**
 * checks.yaml `definitions.luma` (OFIQ, ISO/IEC 29794-5 reference): linearise each 8-bit sRGB
 * channel, Y = 0.2126 R + 0.7152 G + 0.0722 B (BT.709, linear), stored as floor(Y * 255 + 0.5).
 */
export function ofiqLuma(r: number, g: number, b: number): number {
  const y = 0.2126 * (LINEAR[r] ?? 0) + 0.7152 * (LINEAR[g] ?? 0) + 0.0722 * (LINEAR[b] ?? 0);
  return Math.floor(y * 255 + 0.5);
}

export function lumaPlane(buffer: PixelBuffer): LumaPlane {
  const { data, width, height } = buffer;
  const count = width * height;
  const luma = new Float32Array(count);
  const grey = new Float32Array(count);
  const valid = new Uint8Array(count);
  for (let i = 0; i < count; i++) {
    const o = i * 4;
    const r = data[o] ?? 0;
    const g = data[o + 1] ?? 0;
    const b = data[o + 2] ?? 0;
    luma[i] = ofiqLuma(r, g, b);
    grey[i] = 0.299 * r + 0.587 * g + 0.114 * b;
    valid[i] = (data[o + 3] ?? 0) >= OPAQUE ? 1 : 0;
  }
  return { width, height, luma, grey, valid };
}

interface PixelBounds {
  readonly x0: number;
  readonly y0: number;
  readonly x1: number;
  readonly y1: number;
}

/** Pixel bounds [x0, x1) x [y0, y1) of a region inside a crop, clamped to the crop. */
export function regionBounds(region: FaceRegion, layout: CropLayout): PixelBounds {
  const s = layout.interEyeDistance;
  const clampX = (v: number) => Math.min(layout.width, Math.max(0, Math.round(v)));
  const clampY = (v: number) => Math.min(layout.height, Math.max(0, Math.round(v)));
  return {
    x0: clampX(layout.originX + region.left * s),
    x1: clampX(layout.originX + region.right * s),
    y0: clampY(layout.originY + region.top * s),
    y1: clampY(layout.originY + region.bottom * s),
  };
}

export interface ExposureStats {
  /** LUMINANCE_MEAN: mean luma of the region, 0-1. */
  readonly luminanceMean: number;
  /** UNDER_EXPOSURE: fraction of pixels at or below UNDER_EXPOSURE_LEVEL. */
  readonly underExposure: number;
  /** OVER_EXPOSURE: fraction of pixels at or above OVER_EXPOSURE_LEVEL. */
  readonly overExposure: number;
}

/**
 * Exposure statistics over the pixels where `mask` is 1 (for example the face-skin region), or null
 * when the mask is empty or too little of it lies inside the image.
 */
export function exposureStatsInMask(plane: LumaPlane, mask: Uint8Array): ExposureStats | null {
  let total = 0;
  let n = 0;
  let sum = 0;
  let under = 0;
  let over = 0;
  const count = Math.min(mask.length, plane.luma.length);
  for (let i = 0; i < count; i++) {
    if (mask[i] !== 1) continue;
    total++;
    if (plane.valid[i] !== 1) continue;
    const v = plane.luma[i] ?? 0;
    n++;
    sum += v;
    if (v <= UNDER_EXPOSURE_LEVEL) under++;
    if (v >= OVER_EXPOSURE_LEVEL) over++;
  }
  if (total === 0 || n < MIN_COVERAGE * total) return null;
  return { luminanceMean: sum / n / 255, underExposure: under / n, overExposure: over / n };
}

/** Exposure statistics of a rectangular region, or null when too little of it lies inside the image. */
export function exposureStats(plane: LumaPlane, region: FaceRegion, layout: CropLayout): ExposureStats | null {
  const b = regionBounds(region, layout);
  const total = (b.x1 - b.x0) * (b.y1 - b.y0);
  if (total <= 0) return null;
  let n = 0;
  let sum = 0;
  let under = 0;
  let over = 0;
  for (let y = b.y0; y < b.y1; y++) {
    for (let x = b.x0; x < b.x1; x++) {
      const i = y * plane.width + x;
      if (plane.valid[i] !== 1) continue;
      const v = plane.luma[i] ?? 0;
      n++;
      sum += v;
      if (v <= UNDER_EXPOSURE_LEVEL) under++;
      if (v >= OVER_EXPOSURE_LEVEL) over++;
    }
  }
  if (n < MIN_COVERAGE * total) return null;
  return { luminanceMean: sum / n / 255, underExposure: under / n, overExposure: over / n };
}

export interface SharpnessStats {
  /** Contrast-normalised Laplacian energy: std(Laplacian) / (std(luma) + 1). Unbounded; for ranking frames. */
  readonly raw: number;
  /** SHARPNESS estimate, 0 (blurred) to 1 (sharp). Provisional mapping of `raw`. */
  readonly score: number;
}

/**
 * Sharpness from the variance of the Laplacian over a region. Dividing by the region's own contrast
 * keeps dim or low-contrast faces (darker skin, weak light) from reading as blurred.
 */
export function sharpnessStats(plane: LumaPlane, region: FaceRegion, layout: CropLayout): SharpnessStats | null {
  const b = regionBounds(region, layout);
  const total = (b.x1 - b.x0) * (b.y1 - b.y0);
  if (total <= 0) return null;
  // The SDK's own estimator runs on gamma-encoded grey, where its provisional constant was tuned.
  const { width, grey: luma, valid } = plane;
  let lapN = 0;
  let lapSum = 0;
  let lapSq = 0;
  let n = 0;
  let sum = 0;
  let sq = 0;
  for (let y = b.y0; y < b.y1; y++) {
    for (let x = b.x0; x < b.x1; x++) {
      const i = y * width + x;
      if (valid[i] !== 1) continue;
      const v = luma[i] ?? 0;
      n++;
      sum += v;
      sq += v * v;
      if (x <= b.x0 || x >= b.x1 - 1 || y <= b.y0 || y >= b.y1 - 1) continue;
      if (valid[i - 1] !== 1 || valid[i + 1] !== 1 || valid[i - width] !== 1 || valid[i + width] !== 1) continue;
      const lap =
        (luma[i - 1] ?? 0) + (luma[i + 1] ?? 0) + (luma[i - width] ?? 0) + (luma[i + width] ?? 0) - 4 * v;
      lapN++;
      lapSum += lap;
      lapSq += lap * lap;
    }
  }
  if (n < MIN_COVERAGE * total || lapN < 16) return null;
  const lumaStd = Math.sqrt(Math.max(0, sq / n - (sum / n) ** 2));
  const lapStd = Math.sqrt(Math.max(0, lapSq / lapN - (lapSum / lapN) ** 2));
  const raw = lapStd / (lumaStd + 1);
  return { raw, score: raw / (raw + SHARPNESS_HALF_POINT) };
}
