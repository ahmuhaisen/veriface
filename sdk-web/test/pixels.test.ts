import { describe, expect, it } from 'vitest';
import {
  applyAffine,
  cropTransform,
  exposureStats,
  exposureStatsInMask,
  FACE_CROP,
  FACE_REGION,
  lumaPlane,
  ofiqLuma,
  regionBounds,
  sharpnessStats,
  SKIN_REGION,
  srgbToLinear,
} from '../src/measure/pixels';
import { boxBlur, greyImage } from './helpers';

const { width, height } = FACE_CROP;

/** A textured, face-sized test pattern: blobs and edges at several scales. */
function texture(x: number, y: number, contrast = 1): number {
  const v =
    Math.sin(x * 0.9) * Math.cos(y * 0.7) * 40 +
    Math.sin(x * 0.25 + y * 0.3) * 30 +
    ((Math.floor(x / 6) + Math.floor(y / 6)) % 2 === 0 ? 25 : -25);
  return 128 + v * contrast;
}

describe('contract luma (OFIQ: linear sRGB, BT.709 weights, floor(Y * 255 + 0.5))', () => {
  it('linearises with the sRGB curve', () => {
    expect(srgbToLinear(0)).toBe(0);
    expect(srgbToLinear(255)).toBeCloseTo(1, 12);
    expect(srgbToLinear(10)).toBeCloseTo(10 / 255 / 12.92, 12); // linear segment
    expect(srgbToLinear(128)).toBeCloseTo(0.2158605, 6);
  });

  it('weights the linear channels 0.2126 / 0.7152 / 0.0722 and rounds half up', () => {
    expect(ofiqLuma(0, 0, 0)).toBe(0);
    expect(ofiqLuma(255, 255, 255)).toBe(255);
    expect(ofiqLuma(128, 128, 128)).toBe(55);
    expect(ofiqLuma(255, 0, 0)).toBe(Math.floor(0.2126 * 255 + 0.5));
    expect(ofiqLuma(0, 255, 0)).toBe(Math.floor(0.7152 * 255 + 0.5));
    expect(ofiqLuma(0, 0, 255)).toBe(Math.floor(0.0722 * 255 + 0.5));
  });
});

describe('exposure statistics', () => {
  it('measures the mean contract luma of the skin region (0-1)', () => {
    const stats = exposureStats(lumaPlane(greyImage(width, height, () => 128)), SKIN_REGION, FACE_CROP);
    expect(stats?.luminanceMean).toBeCloseTo(55 / 255, 6);
    expect(stats?.underExposure).toBe(0);
    expect(stats?.overExposure).toBe(0);
  });

  it('counts clipped bright pixels (glare) and dark pixels', () => {
    const b = regionBounds(FACE_REGION, FACE_CROP);
    const midX = Math.floor((b.x0 + b.x1) / 2);
    const image = greyImage(width, height, (x) => (x < midX ? 255 : 10));
    const stats = exposureStats(lumaPlane(image), FACE_REGION, FACE_CROP);
    const expected = (midX - b.x0) / (b.x1 - b.x0);
    expect(stats?.overExposure).toBeCloseTo(expected, 6);
    expect(stats?.underExposure).toBeCloseTo(1 - expected, 6);
  });

  it('applies the levels on the linear scale (luma <= 25 dark, >= 247 clipped)', () => {
    const at = (v: number) => exposureStats(lumaPlane(greyImage(width, height, () => v)), SKIN_REGION, FACE_CROP);
    // sRGB grey 89 is linear luma 25 and grey 90 is 26; grey 252 is linear 248 and 251 is 246.
    expect(at(89)?.underExposure).toBe(1);
    expect(at(90)?.underExposure).toBe(0);
    expect(at(252)?.overExposure).toBe(1);
    expect(at(251)?.overExposure).toBe(0);
  });

  it('ignores pixels outside the source image and gives up when too few remain', () => {
    const b = regionBounds(SKIN_REGION, FACE_CROP);
    const halfTransparent = greyImage(
      width,
      height,
      (x) => (x < (b.x0 + b.x1) / 2 ? 200 : 50),
      (x) => (x < (b.x0 + b.x1) / 2 ? 255 : 0),
    );
    expect(exposureStats(lumaPlane(halfTransparent), SKIN_REGION, FACE_CROP)).toBeNull();

    const mostlyOpaque = greyImage(width, height, () => 200, (x) => (x < b.x0 + 2 ? 0 : 255));
    expect(exposureStats(lumaPlane(mostlyOpaque), SKIN_REGION, FACE_CROP)?.luminanceMean).toBeCloseTo(
      ofiqLuma(200, 200, 200) / 255,
      6,
    );
  });

  it('measures within a pixel mask, honouring coverage', () => {
    const plane = lumaPlane(greyImage(width, height, (x) => (x < 20 ? 255 : 0), (x) => (x >= 130 ? 0 : 255)));
    const mask = new Uint8Array(width * height);
    for (let y = 0; y < 10; y++) for (let x = 10; x < 30; x++) mask[y * width + x] = 1;
    expect(exposureStatsInMask(plane, mask)).toEqual({ luminanceMean: 0.5, underExposure: 0.5, overExposure: 0.5 });
    expect(exposureStatsInMask(plane, new Uint8Array(width * height))).toBeNull();
    const outside = new Uint8Array(width * height);
    for (let x = 125; x < 140; x++) outside[x] = 1; // 2/3 outside the image
    expect(exposureStatsInMask(plane, outside)).toBeNull();
  });
});

describe('crop transform', () => {
  it('maps the eye midpoint to the origin and scales and de-rotates the eye line', () => {
    const t = cropTransform({ x: 600, y: 300 }, 30, 100, FACE_CROP);
    const origin = applyAffine(t, { x: 600, y: 300 });
    expect(origin.x).toBeCloseTo(FACE_CROP.originX, 9);
    expect(origin.y).toBeCloseTo(FACE_CROP.originY, 9);
    const angle = (30 * Math.PI) / 180;
    const eye = applyAffine(t, { x: 600 + 50 * Math.cos(angle), y: 300 + 50 * Math.sin(angle) });
    expect(eye.x).toBeCloseTo(FACE_CROP.originX + FACE_CROP.interEyeDistance / 2, 9);
    expect(eye.y).toBeCloseTo(FACE_CROP.originY, 9);
  });
});

describe('sharpness (contrast-normalised Laplacian)', () => {
  const sharp = greyImage(width, height, (x, y) => texture(x, y));

  it('decreases monotonically as the face gets blurrier', () => {
    const raw = [0, 1, 2, 3, 5].map((radius) => sharpnessStats(lumaPlane(boxBlur(sharp, radius)), FACE_REGION, FACE_CROP)?.raw ?? NaN);
    for (let i = 1; i < raw.length; i++) expect(raw[i]).toBeLessThan(raw[i - 1] ?? 0);
  });

  it('maps to a 0-1 score that ranks the same way', () => {
    const crisp = sharpnessStats(lumaPlane(sharp), FACE_REGION, FACE_CROP);
    const soft = sharpnessStats(lumaPlane(boxBlur(sharp, 3)), FACE_REGION, FACE_CROP);
    expect(crisp?.score).toBeGreaterThan(0);
    expect(crisp?.score).toBeLessThan(1);
    expect(crisp?.score ?? 0).toBeGreaterThan(soft?.score ?? 1);
  });

  it('does not call a low-contrast face blurry: contrast is normalised out', () => {
    const highContrast = sharpnessStats(lumaPlane(sharp), FACE_REGION, FACE_CROP)?.raw ?? 0;
    const lowContrast =
      sharpnessStats(lumaPlane(greyImage(width, height, (x, y) => texture(x, y, 0.35))), FACE_REGION, FACE_CROP)?.raw ?? 0;
    expect(lowContrast / highContrast).toBeGreaterThan(0.85);
    expect(lowContrast / highContrast).toBeLessThan(1.15);
  });

  it('is zero for a flat image', () => {
    expect(sharpnessStats(lumaPlane(greyImage(width, height, () => 90)), FACE_REGION, FACE_CROP)?.raw).toBe(0);
  });

  it('returns null when the face lies mostly outside the image', () => {
    const empty = greyImage(width, height, () => 90, () => 0);
    expect(sharpnessStats(lumaPlane(empty), FACE_REGION, FACE_CROP)).toBeNull();
  });
});

describe('crop layout', () => {
  it('places regions in inter-eye-distance units around the eye midpoint', () => {
    expect(regionBounds({ left: -0.5, right: 0.5, top: 0, bottom: 1 }, FACE_CROP)).toEqual({
      x0: FACE_CROP.originX - 25,
      x1: FACE_CROP.originX + 25,
      y0: FACE_CROP.originY,
      y1: FACE_CROP.originY + 50,
    });
  });

  it('keeps the face regions inside the crop', () => {
    for (const region of [SKIN_REGION, FACE_REGION]) {
      const b = regionBounds(region, FACE_CROP);
      expect(b.x0).toBeGreaterThanOrEqual(0);
      expect(b.y0).toBeGreaterThanOrEqual(0);
      expect(b.x1).toBeLessThanOrEqual(FACE_CROP.width);
      expect(b.y1).toBeLessThanOrEqual(FACE_CROP.height);
    }
  });
});
