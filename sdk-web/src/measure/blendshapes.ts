/** A blendshape coefficient as returned by MediaPipe (`Category`). */
export interface BlendshapeScore {
  readonly categoryName: string;
  readonly score: number;
}

/**
 * EYES_OPEN value: openness of the less-open eye, 0 (closed) to 1 (open), from the `eyeBlinkLeft`
 * and `eyeBlinkRight` blendshapes. Returns null when either is missing.
 *
 * MediaPipe's left/right blendshape labels swap when the frame is mirrored (and they follow the
 * model's own convention, not necessarily the subject's). The SDK never mirrors the frames it
 * analyses, and this value uses the worse of the two eyes, so it does not depend on which eye a
 * label refers to.
 */
export function eyesOpenFromBlendshapes(categories: readonly BlendshapeScore[]): number | null {
  const left = scoreOf(categories, 'eyeBlinkLeft');
  const right = scoreOf(categories, 'eyeBlinkRight');
  if (left === null || right === null) return null;
  return clamp01(1 - Math.max(left, right));
}

function scoreOf(categories: readonly BlendshapeScore[], name: string): number | null {
  const found = categories.find((c) => c.categoryName === name);
  return found !== undefined && Number.isFinite(found.score) ? found.score : null;
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}
