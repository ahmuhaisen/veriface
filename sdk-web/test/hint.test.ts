import { describe, expect, it } from 'vitest';
import { HintSmoother } from '../src/guidance/hint';
import { SELFIE_GUIDANCE_CODES, type SelfieGuidanceCode } from '../src/guidance/types';

const rank = (code: SelfieGuidanceCode) => SELFIE_GUIDANCE_CODES.indexOf(code);

describe('hint smoothing', () => {
  it('shows the most frequent code of the window', () => {
    const s = new HintSmoother<SelfieGuidanceCode>(400, rank);
    s.update('TOO_FAR', 0);
    s.update('TOO_FAR', 70);
    expect(s.update('YAW_OUT_OF_RANGE', 140)).toBe('TOO_FAR');
    s.update('YAW_OUT_OF_RANGE', 210);
    s.update('YAW_OUT_OF_RANGE', 280);
    expect(s.update('YAW_OUT_OF_RANGE', 350)).toBe('YAW_OUT_OF_RANGE');
  });

  it('prefers the real problem over "hold still" when a value sits on a threshold', () => {
    const s = new HintSmoother<SelfieGuidanceCode>(400, rank);
    let shown: SelfieGuidanceCode = 'HOLD_STILL';
    for (let i = 0; i < 10; i++) shown = s.update(i % 2 === 0 ? 'HOLD_STILL' : 'YAW_OUT_OF_RANGE', i * 66);
    expect(shown).toBe('YAW_OUT_OF_RANGE');
  });

  it('forgets codes older than the window', () => {
    const s = new HintSmoother<SelfieGuidanceCode>(400, rank);
    for (let i = 0; i < 6; i++) s.update('NO_FACE', i * 50);
    expect(s.update('HOLD_STILL', 1000)).toBe('HOLD_STILL');
  });

  it('shows immediate codes at once', () => {
    const s = new HintSmoother<SelfieGuidanceCode>(400, rank, new Set(['READY'] as const));
    for (let i = 0; i < 6; i++) s.update('HOLD_STILL', i * 50);
    expect(s.update('READY', 300)).toBe('READY');
  });

  it('passes codes through when smoothing is disabled', () => {
    const s = new HintSmoother<SelfieGuidanceCode>(0, rank);
    s.update('NO_FACE', 0);
    expect(s.update('BLURRY', 10)).toBe('BLURRY');
  });
});
