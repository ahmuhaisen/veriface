import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parse } from 'yaml';
import type { DocumentMeasurements, DocumentPortrait } from '../src/guidance/document';
import type { SelfieFace, SelfieMeasurements } from '../src/guidance/selfie';
import type { NormalizedPoint } from '../src/measure/face-landmarks';
import type { PixelBuffer } from '../src/measure/pixels';

/** Reads a YAML file from the repository root (contracts/, profiles/). */
export function readRepoYaml(path: string): unknown {
  return parse(readFileSync(fileURLToPath(new URL(`../../${path}`, import.meta.url)), 'utf8'));
}

export function readSdkFile(path: string): string {
  return readFileSync(fileURLToPath(new URL(`../${path}`, import.meta.url)), 'utf8');
}

/** A good, centred, frontal selfie face in a 1280x720 frame. */
export function selfieFace(overrides: Partial<SelfieFace> = {}): SelfieFace {
  return {
    interEyeDistance: 150,
    eyeMidpoint: { x: 640, y: 330 },
    headBox: { x: 440, y: 80, width: 400, height: 560 },
    headInFrame: 80 / 560,
    yaw: 0,
    pitch: 0,
    roll: 0,
    eyesOpen: 0.9,
    luminanceMean: 0.5,
    underExposure: 0.01,
    overExposure: 0.01,
    sharpness: 0.7,
    sharpnessRaw: 0.6,
    ...overrides,
  };
}

export function selfieMeasurements(
  face: SelfieFace | null = selfieFace(),
  extra: Partial<SelfieMeasurements> = {},
): SelfieMeasurements {
  return {
    frameWidth: 1280,
    frameHeight: 720,
    faceInterEyeDistances: face ? [face.interEyeDistance] : [],
    face,
    ...extra,
  };
}

/** A sharp, well-lit document portrait in a 1920x1080 frame. */
export function documentPortrait(overrides: Partial<DocumentPortrait> = {}): DocumentPortrait {
  return {
    interEyeDistance: 80,
    eyeMidpoint: { x: 700, y: 500 },
    box: { x: 620, y: 420, width: 160, height: 200 },
    luminanceMean: 0.55,
    underExposure: 0.02,
    overExposure: 0.01,
    sharpness: 0.6,
    sharpnessRaw: 0.4,
    ...overrides,
  };
}

export function documentMeasurements(
  portrait: DocumentPortrait | null = documentPortrait(),
  extra: Partial<DocumentMeasurements> = {},
): DocumentMeasurements {
  return {
    frameWidth: 1920,
    frameHeight: 1080,
    guide: { x: 247, y: 99, width: 1426, height: 882 },
    portraitInterEyeDistances: portrait ? [portrait.interEyeDistance] : [],
    portrait,
    ...extra,
  };
}

/**
 * Synthetic 478-point face mesh in normalised coordinates: points on an upright ellipse, with the
 * iris centres (468, 473) placed on the eye line. Rotated clockwise by `rollDegrees` around the
 * eye midpoint (image y-down).
 */
export function syntheticMesh(options: {
  width: number;
  height: number;
  eyeMidpoint: { x: number; y: number };
  interEyeDistance: number;
  rollDegrees?: number;
}): NormalizedPoint[] {
  const { width, height, eyeMidpoint, interEyeDistance } = options;
  const angle = ((options.rollDegrees ?? 0) * Math.PI) / 180;
  const cos = Math.cos(angle);
  const sin = Math.sin(angle);
  const place = (dx: number, dy: number): NormalizedPoint => ({
    x: (eyeMidpoint.x + dx * cos - dy * sin) / width,
    y: (eyeMidpoint.y + dx * sin + dy * cos) / height,
  });
  // Face oval: 2.3 IED wide; from 1.2 IED above to 2.0 IED below the eye line.
  const halfWidth = 1.15 * interEyeDistance;
  const top = -1.2 * interEyeDistance;
  const bottom = 2.0 * interEyeDistance;
  const centreY = (top + bottom) / 2;
  const halfHeight = (bottom - top) / 2;
  const points: NormalizedPoint[] = [];
  for (let i = 0; i < 478; i++) {
    const t = (i / 478) * 2 * Math.PI;
    points.push(place(halfWidth * Math.cos(t), centreY + halfHeight * Math.sin(t)));
  }
  points[468] = place(-interEyeDistance / 2, 0);
  points[473] = place(interEyeDistance / 2, 0);
  return points;
}

/** RGBA buffer filled by a luma function (grey pixels, fully opaque unless `alpha` says otherwise). */
export function greyImage(
  width: number,
  height: number,
  luma: (x: number, y: number) => number,
  alpha: (x: number, y: number) => number = () => 255,
): PixelBuffer {
  const data = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const o = (y * width + x) * 4;
      const v = luma(x, y);
      data[o] = v;
      data[o + 1] = v;
      data[o + 2] = v;
      data[o + 3] = alpha(x, y);
    }
  }
  return { data, width, height };
}

/** Separable box blur of a grey image (radius in pixels), used to fabricate blurred frames. */
export function boxBlur(image: PixelBuffer, radius: number): PixelBuffer {
  if (radius <= 0) return image;
  const { width, height } = image;
  const src = new Float32Array(width * height);
  for (let i = 0; i < width * height; i++) src[i] = image.data[i * 4] ?? 0;
  const pass = (input: Float32Array, horizontal: boolean): Float32Array => {
    const out = new Float32Array(width * height);
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        let sum = 0;
        let n = 0;
        for (let k = -radius; k <= radius; k++) {
          const xx = horizontal ? x + k : x;
          const yy = horizontal ? y : y + k;
          if (xx < 0 || yy < 0 || xx >= width || yy >= height) continue;
          sum += input[yy * width + xx] ?? 0;
          n++;
        }
        out[y * width + x] = sum / n;
      }
    }
    return out;
  };
  const blurred = pass(pass(src, true), false);
  return greyImage(width, height, (x, y) => blurred[y * width + x] ?? 0);
}

/** Records retain/release calls per payload to prove that nothing leaks or is double-freed. */
export class RefLedger {
  readonly retained = new Map<number, number>();
  readonly released = new Map<number, number>();

  retain = (id: number): void => {
    this.retained.set(id, (this.retained.get(id) ?? 0) + 1);
  };

  release = (id: number): void => {
    this.released.set(id, (this.released.get(id) ?? 0) + 1);
  };

  /** Payloads currently held (retained more often than released). */
  held(): number[] {
    return [...this.retained.keys()].filter((id) => (this.retained.get(id) ?? 0) > (this.released.get(id) ?? 0));
  }

  overReleased(): number[] {
    return [...this.released.keys()].filter((id) => (this.released.get(id) ?? 0) > (this.retained.get(id) ?? 0));
  }
}
