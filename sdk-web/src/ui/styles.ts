/**
 * Shadow DOM styles. Theme with CSS custom properties on the host, or style parts with ::part():
 * root, stage, video, still, overlay, hint, error, actions, capture-button.
 */
export const STYLES = /* css */ `
:host {
  --dfc-accent: #15803d;
  --dfc-accent-contrast: #ffffff;
  --dfc-outline: rgba(255, 255, 255, 0.95);
  --dfc-shade: rgba(15, 23, 42, 0.62);
  --dfc-radius: 16px;
  --dfc-error: #b91c1c;
  display: block;
  position: relative;
  max-width: 520px;
}
:host([hidden]) { display: none; }
.root { display: flex; flex-direction: column; gap: 12px; }
.stage {
  position: relative;
  overflow: hidden;
  border-radius: var(--dfc-radius);
  background: #000;
  aspect-ratio: 3 / 4;
}
.root[data-mode='document'] .stage { aspect-ratio: 16 / 9; }
video, .still {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: cover;
}
.root[data-mode='document'] video, .root[data-mode='document'] .still { object-fit: contain; }
.still[hidden] { display: none; }
/* Explicit stacking: a transformed (mirrored) video is composited on its own layer, and some
   engines then paint it over later siblings without z-index. */
video { z-index: 0; }
.still { z-index: 1; }
.overlay { position: absolute; inset: 0; z-index: 2; width: 100%; height: 100%; pointer-events: none; }
.shade { fill: var(--dfc-shade); }
.frame { fill: none; stroke: var(--dfc-outline); stroke-width: 3; }
.progress {
  fill: none;
  stroke: var(--dfc-accent);
  stroke-width: 6;
  stroke-linecap: round;
  transition: stroke-dasharray 150ms linear;
}
.root[data-state='hold'] .frame, .root[data-state='ready'] .frame { stroke: var(--dfc-accent); }
.root[data-state='captured'] .overlay, .root[data-state='error'] .overlay { display: none; }
.hint { margin: 0; min-height: 1.5em; text-align: center; font-weight: 600; line-height: 1.5; }
.error { margin: 0; color: var(--dfc-error); text-align: center; line-height: 1.5; }
.error[hidden], .capture[hidden] { display: none; }
.actions { display: flex; justify-content: center; }
.capture {
  font: inherit;
  font-weight: 600;
  min-height: 44px;
  padding: 10px 24px;
  border: none;
  border-radius: 999px;
  background: var(--dfc-accent);
  color: var(--dfc-accent-contrast);
  cursor: pointer;
}
.capture:focus-visible { outline: 3px solid var(--dfc-accent); outline-offset: 3px; }
@media (prefers-reduced-motion: reduce) {
  .progress { transition: none; }
}
`;
