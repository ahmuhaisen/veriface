/**
 * Smooths the hint shown to the user. Per-frame guidance can flicker when a value sits on a
 * threshold; the shown hint is the most frequent code of the last `windowMs`. Ties go to the code
 * with the lower rank (pass the guidance priority, so a real problem beats "hold still").
 * Codes in `immediate` (for example READY) are shown at once.
 */
export class HintSmoother<C extends string> {
  private readonly history: { code: C; at: number }[] = [];

  constructor(
    private readonly windowMs: number,
    private readonly rank: (code: C) => number,
    private readonly immediate: ReadonlySet<C> = new Set<C>(),
  ) {}

  update(code: C, now: number): C {
    if (this.immediate.has(code) || this.windowMs <= 0) {
      this.history.length = 0;
      this.history.push({ code, at: now });
      return code;
    }
    this.history.push({ code, at: now });
    while (this.history.length > 1 && (this.history[0]?.at ?? now) < now - this.windowMs) this.history.shift();
    const counts = new Map<C, number>();
    for (const entry of this.history) counts.set(entry.code, (counts.get(entry.code) ?? 0) + 1);
    let shown = code;
    let shownCount = 0;
    for (const [candidate, count] of counts) {
      if (count > shownCount || (count === shownCount && this.rank(candidate) < this.rank(shown))) {
        shown = candidate;
        shownCount = count;
      }
    }
    return shown;
  }

  reset(): void {
    this.history.length = 0;
  }
}
