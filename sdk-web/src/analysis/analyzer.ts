/**
 * MediaPipe-backed frame analyzers. This module runs unchanged inside the Web Worker and, as a
 * fallback, on the main thread. It turns model output and pixels into plain measurements; all
 * threshold decisions happen later, in the pure guidance functions.
 */
import { FaceDetector, FaceLandmarker, FilesetResolver } from '@mediapipe/tasks-vision';
import { FaceCaptureError } from '../errors';
import type { DocumentMeasurements, DocumentPortrait } from '../guidance/document';
import type { SelfieFace, SelfieMeasurements } from '../guidance/selfie';
import { eyesOpenFromBlendshapes } from '../measure/blendshapes';
import { cardGuide, gridTiles, mergeOverlapping, refinementCrop, searchRegion } from '../measure/card';
import { eyeLineRoll, faceGeometry, headInFrame } from '../measure/face-landmarks';
import { faceSkinPolygons, polygonMask, type RegionPolygons } from '../measure/face-regions';
import { distance, midpoint, type Point, type Rect } from '../measure/geometry';
import {
  applyAffine,
  cropTransform,
  exposureStats,
  exposureStatsInMask,
  FACE_CROP,
  FACE_REGION,
  lumaPlane,
  sharpnessStats,
  SKIN_REGION,
  type Affine,
  type ExposureStats,
  type SharpnessStats,
} from '../measure/pixels';
import { landmarksToPixels, poseFromLandmarks } from '../measure/pose';
import { fetchModel } from './assets';
import { createSurface, resizeSurface, sampleAlignedCrop, type Surface } from './canvas';
import type { Analyzer, AnalyzerConfig, FrameAnalysis } from './types';

type WasmFileset = Awaited<ReturnType<typeof FilesetResolver.forVisionTasks>>;

/** Two faces are enough to tell "one" from "several" (SINGLE_FACE). */
const MAX_FACES = 2;
/** The document portrait search region is scaled down to at most this many pixels per side. */
const DETECTOR_MAX_SIDE = 320;

export async function createAnalyzer(config: AnalyzerConfig): Promise<Analyzer> {
  const model = await fetchModel(config.modelUrl, config.modelSha256);
  let fileset: WasmFileset;
  try {
    fileset = await FilesetResolver.forVisionTasks(config.wasmBaseUrl);
  } catch (error) {
    throw new FaceCaptureError('asset-load-failed', 'Could not resolve the MediaPipe Wasm files', { cause: error });
  }
  try {
    if (config.mode === 'selfie') {
      // No facial transformation matrix: checks.yaml definitions.pose forbids its perspective model;
      // pose comes from our weak-perspective fit of the landmarks.
      const landmarker = await FaceLandmarker.createFromOptions(fileset, {
        baseOptions: { modelAssetBuffer: model, delegate: config.delegate },
        runningMode: 'VIDEO',
        numFaces: MAX_FACES,
        outputFaceBlendshapes: true,
      });
      return new SelfieAnalyzer(landmarker);
    }
    // IMAGE mode: each frame needs several detector passes (card region, tiles, refinement).
    const detector = await FaceDetector.createFromOptions(fileset, {
      baseOptions: { modelAssetBuffer: model, delegate: config.delegate },
      runningMode: 'IMAGE',
    });
    return new DocumentAnalyzer(detector);
  } catch (error) {
    throw new FaceCaptureError('asset-load-failed', 'Could not start the MediaPipe face model', { cause: error });
  }
}

/** MediaPipe requires strictly increasing timestamps in VIDEO mode. */
class Clock {
  private last = -Infinity;
  next(timestamp: number): number {
    this.last = Math.max(timestamp, this.last + 1);
    return this.last;
  }
}

interface PixelStats {
  readonly skin: ExposureStats | null;
  readonly face: ExposureStats | null;
  readonly sharpness: SharpnessStats | null;
}

/**
 * Pixel statistics on the aligned face crop. `skinRegion` (image pixels) is the face-skin region of
 * checks.yaml `definitions.faceSkinRegion`; without landmarks (document portraits found by the face
 * detector) a fixed cheek-and-nose rectangle stands in for it.
 */
function measurePixels(
  surface: Surface,
  frame: CanvasImageSource,
  transform: Affine,
  skinRegion: RegionPolygons | null,
): PixelStats {
  const plane = lumaPlane(sampleAlignedCrop(surface, frame, transform, FACE_CROP));
  let skin: ExposureStats | null;
  if (skinRegion !== null) {
    const toCrop = (polygon: readonly Point[]) => polygon.map((p) => applyAffine(transform, p));
    const mask = polygonMask(FACE_CROP.width, FACE_CROP.height, {
      include: skinRegion.include.map(toCrop),
      exclude: skinRegion.exclude.map(toCrop),
    });
    skin = exposureStatsInMask(plane, mask);
  } else {
    skin = exposureStats(plane, SKIN_REGION, FACE_CROP);
  }
  return {
    skin,
    face: exposureStats(plane, FACE_REGION, FACE_CROP),
    sharpness: sharpnessStats(plane, FACE_REGION, FACE_CROP),
  };
}

class SelfieAnalyzer implements Analyzer {
  private readonly clock = new Clock();
  private readonly surface = createSurface(FACE_CROP.width, FACE_CROP.height);

  constructor(private readonly landmarker: FaceLandmarker) {}

  analyze(frame: ImageBitmap, timestamp: number): Promise<FrameAnalysis> {
    const start = performance.now();
    const width = frame.width;
    const height = frame.height;
    const result = this.landmarker.detectForVideo(frame, this.clock.next(timestamp));
    const detected = performance.now();

    const faces = result.faceLandmarks
      .map((landmarks, index) => ({ index, geometry: faceGeometry(landmarks, width, height) }))
      .filter((f): f is { index: number; geometry: NonNullable<typeof f.geometry> } => f.geometry !== null);
    let primary: (typeof faces)[number] | undefined;
    for (const f of faces) {
      if (primary === undefined || f.geometry.interEyeDistance > primary.geometry.interEyeDistance) primary = f;
    }

    let face: SelfieFace | null = null;
    if (primary !== undefined) {
      const g = primary.geometry;
      const points = landmarksToPixels(result.faceLandmarks[primary.index] ?? [], width, height);
      // Yaw, pitch and roll all come from the weak-perspective fit (checks.yaml definitions.pose).
      const pose = poseFromLandmarks(points);
      const transform = cropTransform(g.eyeMidpoint, g.roll, g.interEyeDistance, FACE_CROP);
      const pixels = measurePixels(this.surface, frame, transform, faceSkinPolygons(points));
      face = {
        interEyeDistance: g.interEyeDistance,
        eyeMidpoint: g.eyeMidpoint,
        headBox: g.head?.box ?? null,
        headInFrame: g.head ? headInFrame(g.head, width, height) : null,
        yaw: pose?.yaw ?? null,
        pitch: pose?.pitch ?? null,
        roll: pose?.roll ?? null,
        eyesOpen: eyesOpenFromBlendshapes(result.faceBlendshapes[primary.index]?.categories ?? []),
        luminanceMean: pixels.skin?.luminanceMean ?? null,
        underExposure: pixels.skin?.underExposure ?? null,
        overExposure: pixels.skin?.overExposure ?? null,
        sharpness: pixels.sharpness?.score ?? null,
        sharpnessRaw: pixels.sharpness?.raw ?? null,
      };
    }
    const end = performance.now();
    const measurements: SelfieMeasurements = {
      frameWidth: width,
      frameHeight: height,
      faceInterEyeDistances: faces.map((f) => f.geometry.interEyeDistance),
      face,
    };
    return Promise.resolve({
      mode: 'selfie',
      measurements,
      timing: { detectMs: detected - start, pixelsMs: end - detected, totalMs: end - start },
    });
  }

  close(): void {
    this.landmarker.close();
  }
}

interface PortraitCandidate {
  readonly eyes: { readonly a: Point; readonly b: Point };
  readonly interEyeDistance: number;
  readonly box: Rect;
}

/** Tiles searched when the whole card region shows no portrait (a small or distant card). */
const TILE_GRID = 3;
const TILE_FRACTION = 0.45;
/** The refinement crop is this many times the portrait's face box. */
const REFINE_CONTEXT = 2.5;
/** A refined measurement further than this factor from the coarse one is treated as a mismatch. */
const REFINE_TOLERANCE = 1.6;

class DocumentAnalyzer implements Analyzer {
  private readonly surface = createSurface(FACE_CROP.width, FACE_CROP.height);
  private readonly search = createSurface(DETECTOR_MAX_SIDE, DETECTOR_MAX_SIDE);

  constructor(private readonly detector: FaceDetector) {}

  analyze(frame: ImageBitmap, _timestamp: number, guideRect: Rect | null): Promise<FrameAnalysis> {
    const start = performance.now();
    const width = frame.width;
    const height = frame.height;
    const guide = guideRect ?? cardGuide(width, height);
    const region = searchRegion(guide, width, height) ?? { x: 0, y: 0, width, height };

    // 1. The whole card region. 2. If nothing is found, overlapping tiles: the detector shrinks its
    // input to 128 px, so the portrait of a small, distant card only shows up in a tile.
    let candidates = this.detectIn(frame, region);
    if (candidates.length === 0) {
      candidates = mergeOverlapping(gridTiles(region, TILE_GRID, TILE_FRACTION).flatMap((t) => this.detectIn(frame, t)));
    }
    // The largest face is the portrait; smaller ones are ghost portraits or holograms.
    let coarse: PortraitCandidate | undefined;
    for (const c of candidates) {
      if (coarse === undefined || c.interEyeDistance > coarse.interEyeDistance) coarse = c;
    }
    // 3. Re-detect on a crop around the portrait: eye keypoints are regressed at the model's input
    // resolution, so the zoomed pass measures the inter-eye distance far more precisely.
    const primary = coarse === undefined ? undefined : (this.refine(frame, coarse) ?? coarse);
    const detected = performance.now();

    let portrait: DocumentPortrait | null = null;
    if (primary !== undefined) {
      const eyeMidpoint = midpoint(primary.eyes.a, primary.eyes.b);
      const roll = eyeLineRoll(primary.eyes);
      const transform = cropTransform(eyeMidpoint, roll, primary.interEyeDistance, FACE_CROP);
      // The face detector gives no landmark mesh, so the skin statistics use the fallback rectangle.
      const pixels = measurePixels(this.surface, frame, transform, null);
      portrait = {
        interEyeDistance: primary.interEyeDistance,
        eyeMidpoint,
        box: primary.box,
        luminanceMean: pixels.skin?.luminanceMean ?? null,
        underExposure: pixels.skin?.underExposure ?? null,
        // Glare can sit anywhere on a laminated portrait, so it is measured over the whole face.
        overExposure: pixels.face?.overExposure ?? null,
        sharpness: pixels.sharpness?.score ?? null,
        sharpnessRaw: pixels.sharpness?.raw ?? null,
      };
    }
    const end = performance.now();
    const measurements: DocumentMeasurements = {
      frameWidth: width,
      frameHeight: height,
      guide,
      portraitInterEyeDistances: candidates.map((c) =>
        c === coarse && primary !== undefined ? primary.interEyeDistance : c.interEyeDistance,
      ),
      portrait,
    };
    return Promise.resolve({
      mode: 'document',
      measurements,
      timing: { detectMs: detected - start, pixelsMs: end - detected, totalMs: end - start },
    });
  }

  close(): void {
    this.detector.close();
  }

  /** Runs the detector on one rectangle of the frame; results are in frame pixels. */
  private detectIn(frame: ImageBitmap, rect: Rect): PortraitCandidate[] {
    const scale = Math.min(1, DETECTOR_MAX_SIDE / Math.max(rect.width, rect.height));
    const inputWidth = Math.max(1, Math.round(rect.width * scale));
    const inputHeight = Math.max(1, Math.round(rect.height * scale));
    resizeSurface(this.search, inputWidth, inputHeight);
    const ctx = this.search.context;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, inputWidth, inputHeight);
    ctx.drawImage(frame, rect.x, rect.y, rect.width, rect.height, 0, 0, inputWidth, inputHeight);
    const result = this.detector.detect(this.search.canvas);
    const sx = inputWidth / rect.width;
    const sy = inputHeight / rect.height;
    const candidates: PortraitCandidate[] = [];
    for (const detection of result.detections) {
      const [first, second] = detection.keypoints;
      if (first === undefined || second === undefined) continue;
      // Keypoints are normalised to the detector input, which covers exactly `rect`.
      const a = { x: rect.x + first.x * rect.width, y: rect.y + first.y * rect.height };
      const b = { x: rect.x + second.x * rect.width, y: rect.y + second.y * rect.height };
      const bb = detection.boundingBox;
      const box: Rect = bb
        ? { x: rect.x + bb.originX / sx, y: rect.y + bb.originY / sy, width: bb.width / sx, height: bb.height / sy }
        : { x: Math.min(a.x, b.x), y: Math.min(a.y, b.y), width: Math.abs(b.x - a.x), height: Math.abs(b.y - a.y) };
      const interEyeDistance = distance(a, b);
      if (interEyeDistance > 0) candidates.push({ eyes: { a, b }, interEyeDistance, box });
    }
    return candidates;
  }

  /** Re-detects the portrait on a zoomed crop around it; null when that finds no consistent face. */
  private refine(frame: ImageBitmap, coarse: PortraitCandidate): PortraitCandidate | null {
    const crop = refinementCrop(coarse.box, REFINE_CONTEXT, frame.width, frame.height);
    if (crop === null) return null;
    const centre = { x: coarse.box.x + coarse.box.width / 2, y: coarse.box.y + coarse.box.height / 2 };
    let best: PortraitCandidate | null = null;
    let bestDistance = Infinity;
    for (const c of this.detectIn(frame, crop)) {
      const d = distance(centre, { x: c.box.x + c.box.width / 2, y: c.box.y + c.box.height / 2 });
      if (d < bestDistance) {
        best = c;
        bestDistance = d;
      }
    }
    if (best === null) return null;
    const ratio = best.interEyeDistance / coarse.interEyeDistance;
    return ratio > 1 / REFINE_TOLERANCE && ratio < REFINE_TOLERANCE ? best : null;
  }
}
