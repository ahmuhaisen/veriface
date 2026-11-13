import { describe, expect, it } from 'vitest';
import { eyesOpenFromBlendshapes } from '../src/measure/blendshapes';

const shapes = (left: number, right: number) => [
  { categoryName: '_neutral', score: 0.1 },
  { categoryName: 'eyeBlinkLeft', score: left },
  { categoryName: 'eyeBlinkRight', score: right },
  { categoryName: 'jawOpen', score: 0.02 },
];

describe('EYES_OPEN from blendshapes', () => {
  it('reports the openness of the less-open eye', () => {
    expect(eyesOpenFromBlendshapes(shapes(0.1, 0.05))).toBeCloseTo(0.9, 9);
    expect(eyesOpenFromBlendshapes(shapes(0.05, 0.8))).toBeCloseTo(0.2, 9);
  });

  it('is unaffected by a left/right label swap (as happens with a mirrored frame)', () => {
    expect(eyesOpenFromBlendshapes(shapes(0.7, 0.1))).toBe(eyesOpenFromBlendshapes(shapes(0.1, 0.7)));
  });

  it('clamps to [0, 1]', () => {
    expect(eyesOpenFromBlendshapes(shapes(1.2, 0))).toBe(0);
    expect(eyesOpenFromBlendshapes(shapes(-0.2, -0.1))).toBe(1);
  });

  it('returns null when a blink coefficient is missing or invalid', () => {
    expect(eyesOpenFromBlendshapes([])).toBeNull();
    expect(eyesOpenFromBlendshapes([{ categoryName: 'eyeBlinkLeft', score: 0.1 }])).toBeNull();
    expect(eyesOpenFromBlendshapes(shapes(Number.NaN, 0.1))).toBeNull();
  });
});
