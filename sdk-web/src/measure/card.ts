import { clipRect, type Rect } from './geometry';

/** ISO/IEC 7810 ID-1 card size in millimetres (identity cards, driving licences, bank cards). */
export const ID1_WIDTH_MM = 85.6;
export const ID1_HEIGHT_MM = 53.98;
export const ID1_ASPECT = ID1_WIDTH_MM / ID1_HEIGHT_MM;

/** Default share of the frame that the card guide fills (along its limiting side). */
export const DEFAULT_GUIDE_FILL = 0.85;

/** Extra margin around the guide searched for the portrait, as a fraction of the guide size. */
export const GUIDE_SEARCH_MARGIN = 0.15;

/**
 * The landscape ID-1 card guide, centred in the frame, as large as `fill` allows. It is the same
 * rectangle the overlay draws and the analysis searches, in frame pixels.
 */
export function cardGuide(frameWidth: number, frameHeight: number, fill: number = DEFAULT_GUIDE_FILL): Rect {
  const maxWidth = frameWidth * fill;
  const maxHeight = frameHeight * fill;
  const width = Math.min(maxWidth, maxHeight * ID1_ASPECT);
  const height = width / ID1_ASPECT;
  return { x: (frameWidth - width) / 2, y: (frameHeight - height) / 2, width, height };
}

/** The guide grown by `GUIDE_SEARCH_MARGIN` on every side, clipped to the frame. */
export function searchRegion(guide: Rect, frameWidth: number, frameHeight: number): Rect | null {
  const dx = guide.width * GUIDE_SEARCH_MARGIN;
  const dy = guide.height * GUIDE_SEARCH_MARGIN;
  return clipRect(
    { x: guide.x - dx, y: guide.y - dy, width: guide.width + 2 * dx, height: guide.height + 2 * dy },
    frameWidth,
    frameHeight,
  );
}

/**
 * Overlapping n x n tiles covering `region`, each `fraction` of its width and height. The face
 * detector shrinks its input to 128 px, so a small or distant card's portrait is only found when
 * the detector looks at part of the region at a time.
 */
export function gridTiles(region: Rect, n: number, fraction: number): Rect[] {
  const width = region.width * fraction;
  const height = region.height * fraction;
  const stepX = n > 1 ? (region.width - width) / (n - 1) : 0;
  const stepY = n > 1 ? (region.height - height) / (n - 1) : 0;
  const tiles: Rect[] = [];
  for (let row = 0; row < n; row++) {
    for (let col = 0; col < n; col++) {
      tiles.push({ x: region.x + col * stepX, y: region.y + row * stepY, width, height });
    }
  }
  return tiles;
}

/** Intersection over union of two rectangles. */
export function iou(a: Rect, b: Rect): number {
  const x0 = Math.max(a.x, b.x);
  const y0 = Math.max(a.y, b.y);
  const x1 = Math.min(a.x + a.width, b.x + b.width);
  const y1 = Math.min(a.y + a.height, b.y + b.height);
  const intersection = Math.max(0, x1 - x0) * Math.max(0, y1 - y0);
  const union = a.width * a.height + b.width * b.height - intersection;
  return union > 0 ? intersection / union : 0;
}

/** Drops duplicates of the same face found in overlapping tiles, keeping the larger detection. */
export function mergeOverlapping<T extends { readonly box: Rect; readonly interEyeDistance: number }>(
  items: readonly T[],
  threshold = 0.3,
): T[] {
  const sorted = [...items].sort((a, b) => b.interEyeDistance - a.interEyeDistance);
  const kept: T[] = [];
  for (const item of sorted) {
    if (kept.every((k) => iou(k.box, item.box) < threshold)) kept.push(item);
  }
  return kept;
}

/** A square crop centred on a face box, `context` times its larger side, clipped to the frame. */
export function refinementCrop(box: Rect, context: number, frameWidth: number, frameHeight: number): Rect | null {
  const side = Math.max(box.width, box.height) * context;
  const cx = box.x + box.width / 2;
  const cy = box.y + box.height / 2;
  return clipRect({ x: cx - side / 2, y: cy - side / 2, width: side, height: side }, frameWidth, frameHeight);
}
