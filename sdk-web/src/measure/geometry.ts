/** Image coordinates: origin top-left, x to the right, y down, in pixels (checks.yaml convention). */
export interface Point {
  readonly x: number;
  readonly y: number;
}

export interface Rect {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

export const RAD_TO_DEG = 180 / Math.PI;

export function distance(a: Point, b: Point): number {
  return Math.hypot(b.x - a.x, b.y - a.y);
}

export function midpoint(a: Point, b: Point): Point {
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

/** Rotates `p` around `center` by `angle` radians (positive = clockwise in y-down coordinates). */
export function rotateAround(p: Point, center: Point, angle: number): Point {
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const dx = p.x - center.x;
  const dy = p.y - center.y;
  return { x: center.x + dx * cos - dy * sin, y: center.y + dx * sin + dy * cos };
}

/**
 * Folds an angle in degrees into (-90, 90]. A line has no direction, so an eye line measured from
 * either eye gives the same in-plane rotation.
 */
export function foldHalfTurn(degrees: number): number {
  let a = degrees % 180;
  if (a > 90) a -= 180;
  if (a <= -90) a += 180;
  return a;
}

export function boundingRect(points: readonly Point[]): Rect | null {
  if (points.length === 0) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const p of points) {
    if (p.x < minX) minX = p.x;
    if (p.y < minY) minY = p.y;
    if (p.x > maxX) maxX = p.x;
    if (p.y > maxY) maxY = p.y;
  }
  return { x: minX, y: minY, width: maxX - minX, height: maxY - minY };
}

export function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** Intersection of `rect` with the image area, or null when they do not overlap. */
export function clipRect(rect: Rect, width: number, height: number): Rect | null {
  const x0 = clamp(rect.x, 0, width);
  const y0 = clamp(rect.y, 0, height);
  const x1 = clamp(rect.x + rect.width, 0, width);
  const y1 = clamp(rect.y + rect.height, 0, height);
  if (x1 <= x0 || y1 <= y0) return null;
  return { x: x0, y: y0, width: x1 - x0, height: y1 - y0 };
}
