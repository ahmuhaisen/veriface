import type { Rect } from '../measure/geometry';

export interface Size {
  readonly width: number;
  readonly height: number;
}

export type ObjectFit = 'cover' | 'contain';

export interface Ellipse {
  readonly cx: number;
  readonly cy: number;
  readonly rx: number;
  readonly ry: number;
}

/** Where the video picture lands inside an element for a centred `object-fit`. */
export function fittedContentBox(video: Size, element: Size, fit: ObjectFit): Rect {
  const sx = element.width / video.width;
  const sy = element.height / video.height;
  const scale = fit === 'cover' ? Math.max(sx, sy) : Math.min(sx, sy);
  const width = video.width * scale;
  const height = video.height * scale;
  return { x: (element.width - width) / 2, y: (element.height - height) / 2, width, height };
}

/**
 * Maps a rectangle from video pixels to element pixels. A mirrored preview flips x, so overlays
 * drawn from un-mirrored analysis coordinates line up with what the user sees.
 */
export function videoRectToElement(rect: Rect, video: Size, element: Size, fit: ObjectFit, mirrored: boolean): Rect {
  const box = fittedContentBox(video, element, fit);
  const scale = box.width / video.width;
  const x = mirrored ? box.x + (video.width - rect.x - rect.width) * scale : box.x + rect.x * scale;
  return { x, y: box.y + rect.y * scale, width: rect.width * scale, height: rect.height * scale };
}

/** SVG path data for an ellipse: two arcs, starting at the top and running clockwise. */
export function ellipsePath(e: Ellipse): string {
  const { cx, cy, rx, ry } = e;
  return `M${f(cx)} ${f(cy - ry)}a${f(rx)} ${f(ry)} 0 1 1 0 ${f(2 * ry)}a${f(rx)} ${f(ry)} 0 1 1 0 ${f(-2 * ry)}Z`;
}

/** SVG path data for a rectangle with rounded corners. */
export function roundedRectPath(r: Rect, radius: number): string {
  const k = Math.max(0, Math.min(radius, r.width / 2, r.height / 2));
  const right = r.x + r.width;
  const bottom = r.y + r.height;
  return (
    `M${f(r.x + k)} ${f(r.y)}H${f(right - k)}a${f(k)} ${f(k)} 0 0 1 ${f(k)} ${f(k)}` +
    `V${f(bottom - k)}a${f(k)} ${f(k)} 0 0 1 ${f(-k)} ${f(k)}` +
    `H${f(r.x + k)}a${f(k)} ${f(k)} 0 0 1 ${f(-k)} ${f(-k)}` +
    `V${f(r.y + k)}a${f(k)} ${f(k)} 0 0 1 ${f(k)} ${f(-k)}Z`
  );
}

/**
 * The shade: the whole element minus the guide shape. Drawn with fill-rule evenodd, so it needs no
 * SVG mask (url(#id) references are unreliable inside shadow DOM).
 */
export function shadePath(element: Size, hole: string): string {
  return `M0 0H${f(element.width)}V${f(element.height)}H0Z${hole}`;
}

function f(value: number): string {
  return Number.isFinite(value) ? String(Math.round(value * 100) / 100) : '0';
}

/** Face aspect of the selfie oval (width / height). */
export const OVAL_ASPECT = 0.75;

/** The selfie oval: centred slightly above the middle, about 70% of the element's height. */
export function selfieOval(element: Size): Ellipse {
  const ry = Math.min(element.height * 0.36, (element.width * 0.42) / OVAL_ASPECT);
  return { cx: element.width / 2, cy: element.height * 0.47, rx: ry * OVAL_ASPECT, ry };
}
