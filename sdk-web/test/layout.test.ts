import { describe, expect, it } from 'vitest';
import { cardGuide, gridTiles, ID1_ASPECT, iou, mergeOverlapping, refinementCrop, searchRegion } from '../src/measure/card';
import {
  ellipsePath,
  fittedContentBox,
  OVAL_ASPECT,
  roundedRectPath,
  selfieOval,
  shadePath,
  videoRectToElement,
} from '../src/ui/layout';

describe('ID-1 card guide', () => {
  it('has the ID-1 aspect (85.60 x 53.98 mm), centred', () => {
    const guide = cardGuide(1920, 1080);
    expect(guide.width / guide.height).toBeCloseTo(85.6 / 53.98, 9);
    expect(ID1_ASPECT).toBeCloseTo(1.5858, 4);
    expect(guide.x + guide.width / 2).toBeCloseTo(960, 9);
    expect(guide.y + guide.height / 2).toBeCloseTo(540, 9);
  });

  it('fills the limiting side by the requested share', () => {
    expect(cardGuide(1920, 1080, 0.85).height).toBeCloseTo(0.85 * 1080, 9); // landscape: height-limited
    expect(cardGuide(1080, 1920, 0.85).width).toBeCloseTo(0.85 * 1080, 9); // portrait: width-limited
  });

  it('searches a margin around the guide, clipped to the frame', () => {
    const guide = cardGuide(1280, 720, 0.95);
    const region = searchRegion(guide, 1280, 720);
    expect(region?.x).toBeGreaterThanOrEqual(0);
    expect(region?.y).toBe(0);
    expect((region?.y ?? 0) + (region?.height ?? 0)).toBe(720);
    expect(region?.width).toBeGreaterThan(guide.width);
  });
});

describe('portrait search geometry', () => {
  const region = { x: 100, y: 50, width: 1000, height: 600 };

  it('covers the region with overlapping tiles', () => {
    const tiles = gridTiles(region, 3, 0.45);
    expect(tiles).toHaveLength(9);
    expect(tiles[0]).toEqual({ x: 100, y: 50, width: 450, height: 270 });
    const last = tiles[8];
    expect((last?.x ?? 0) + (last?.width ?? 0)).toBeCloseTo(1100, 9);
    expect((last?.y ?? 0) + (last?.height ?? 0)).toBeCloseTo(650, 9);
    // Neighbouring tiles overlap, so a face on a seam is whole in at least one tile.
    expect(iou(tiles[0] ?? region, tiles[1] ?? region)).toBeGreaterThan(0);
  });

  it('computes intersection over union', () => {
    const a = { x: 0, y: 0, width: 10, height: 10 };
    expect(iou(a, a)).toBe(1);
    expect(iou(a, { x: 5, y: 0, width: 10, height: 10 })).toBeCloseTo(50 / 150, 9);
    expect(iou(a, { x: 20, y: 20, width: 5, height: 5 })).toBe(0);
  });

  it('merges duplicates of one face from overlapping tiles, keeping the larger', () => {
    const face = (x: number, ied: number) => ({ box: { x, y: 0, width: 100, height: 120 }, interEyeDistance: ied });
    const merged = mergeOverlapping([face(0, 40), face(5, 42), face(400, 20)]);
    expect(merged.map((m) => m.interEyeDistance)).toEqual([42, 20]);
  });

  it('crops a square around the face for refinement, clipped to the frame', () => {
    expect(refinementCrop({ x: 500, y: 400, width: 100, height: 120 }, 2.5, 1920, 1080)).toEqual({
      x: 400,
      y: 310,
      width: 300,
      height: 300,
    });
    expect(refinementCrop({ x: 0, y: 0, width: 100, height: 100 }, 2.5, 1920, 1080)).toEqual({
      x: 0,
      y: 0,
      width: 175,
      height: 175,
    });
  });
});

describe('overlay geometry', () => {
  it('computes object-fit boxes', () => {
    const contain = fittedContentBox({ width: 1920, height: 1080 }, { width: 400, height: 400 }, 'contain');
    expect(contain.x).toBeCloseTo(0, 9);
    expect(contain.y).toBeCloseTo(87.5, 9);
    expect(contain.width).toBeCloseTo(400, 9);
    expect(contain.height).toBeCloseTo(225, 9);
    const cover = fittedContentBox({ width: 1280, height: 720 }, { width: 300, height: 400 }, 'cover');
    expect(cover.height).toBeCloseTo(400, 9);
    expect(cover.x).toBeCloseTo((300 - 1280 * (400 / 720)) / 2, 9);
  });

  it('maps video rectangles onto the element, flipping x for a mirrored preview', () => {
    const video = { width: 1000, height: 500 };
    const element = { width: 500, height: 250 };
    const rect = { x: 100, y: 50, width: 200, height: 100 };
    expect(videoRectToElement(rect, video, element, 'contain', false)).toEqual({ x: 50, y: 25, width: 100, height: 50 });
    expect(videoRectToElement(rect, video, element, 'contain', true)).toEqual({ x: 350, y: 25, width: 100, height: 50 });
  });

  it('builds the overlay as plain path data (no SVG mask references)', () => {
    expect(ellipsePath({ cx: 50, cy: 60, rx: 30, ry: 40 })).toBe('M50 20a30 40 0 1 1 0 80a30 40 0 1 1 0 -80Z');
    expect(roundedRectPath({ x: 10, y: 20, width: 100, height: 60 }, 5)).toBe(
      'M15 20H105a5 5 0 0 1 5 5V75a5 5 0 0 1 -5 5H15a5 5 0 0 1 -5 -5V25a5 5 0 0 1 5 -5Z',
    );
    // The shade is the element's outline followed by the hole, filled with evenodd.
    expect(shadePath({ width: 200, height: 100 }, 'M1 1Z')).toBe('M0 0H200V100H0ZM1 1Z');
    // Radii never exceed half the rectangle, and non-finite input cannot reach the markup.
    expect(roundedRectPath({ x: 0, y: 0, width: 10, height: 4 }, 50)).toContain('a2 2');
    expect(ellipsePath({ cx: Number.NaN, cy: 0, rx: 1, ry: 1 })).toMatch(/^M0 /);
  });

  it('keeps the selfie oval inside the element with a face-like aspect', () => {
    for (const element of [
      { width: 360, height: 480 },
      { width: 640, height: 360 },
      { width: 200, height: 600 },
    ]) {
      const o = selfieOval(element);
      expect(o.rx / o.ry).toBeCloseTo(OVAL_ASPECT, 9);
      expect(o.cx - o.rx).toBeGreaterThanOrEqual(0);
      expect(o.cx + o.rx).toBeLessThanOrEqual(element.width);
      expect(o.cy - o.ry).toBeGreaterThanOrEqual(0);
      expect(o.cy + o.ry).toBeLessThanOrEqual(element.height);
    }
  });
});
