// Example page: drives <veriface-capture> and shows its guidance and results. No server calls.
// Event payloads are typed through the element's event map (see VerifaceCaptureEventMap).
import '../../src/element';

function find<T extends Element>(selector: string, type: abstract new () => T): T {
  const element = document.querySelector(selector);
  if (!(element instanceof type)) throw new Error(`Missing ${selector}`);
  return element;
}

const capture = document.querySelector('veriface-capture');
if (capture === null) throw new Error('Missing <veriface-capture>');
const ui = {
  status: find('#status', HTMLElement),
  code: find('#code', HTMLElement),
  frameCode: find('#frame-code', HTMLElement),
  engine: find('#engine', HTMLElement),
  analysis: find('#analysis', HTMLElement),
  progress: find('#progress', HTMLProgressElement),
  issues: find('#issues', HTMLElement),
  advisories: find('#advisories', HTMLElement),
  error: find('#error', HTMLElement),
  measurements: find('#measurements', HTMLElement),
  resultMeta: find('#result-meta', HTMLElement),
  resultImage: find('#result-image', HTMLImageElement),
  burst: find('#burst', HTMLElement),
  summary: find('#summary', HTMLElement),
  signals: find('#signals', HTMLElement),
};

// Ask for a short burst too, to show what a backend could use for replay analysis.
capture.options = { burstCount: 3 };

const objectUrls: string[] = [];
const json = (value: unknown) =>
  JSON.stringify(value, (_key, v: unknown) => (typeof v === 'number' ? Math.round(v * 1000) / 1000 : v), 2);

find('#start', HTMLButtonElement).addEventListener('click', () => {
  capture.start().catch(() => undefined); // failures arrive as 'error' events
});
find('#stop', HTMLButtonElement).addEventListener('click', () => {
  capture.stop();
});

for (const input of document.querySelectorAll<HTMLInputElement>('input[name="mode"]')) {
  input.addEventListener('change', () => {
    capture.mode = input.value === 'document' ? 'document' : 'selfie';
  });
}
for (const input of document.querySelectorAll<HTMLInputElement>('input[name="lang"]')) {
  input.addEventListener('change', () => {
    capture.setAttribute('lang', input.value);
  });
}

capture.addEventListener('statuschange', (event) => {
  ui.status.textContent = event.detail;
});

capture.addEventListener('guidance', (event) => {
  const state = event.detail;
  ui.code.textContent = `${state.code} (${state.messageKey})`;
  ui.frameCode.textContent = state.frameCode;
  ui.analysis.textContent = `${state.frame.width}x${state.frame.height}, ${state.frame.analysisMs.toFixed(1)} ms`;
  ui.progress.value = state.stability.progress;
  ui.issues.textContent = state.issues.map((i) => `${i.code} [${i.checkId}]`).join(', ') || 'none';
  ui.advisories.textContent = state.advisories.map((i) => `${i.code} [${i.checkId}]`).join(', ') || 'none';
  ui.measurements.textContent = json(state.measurements);
  ui.error.hidden = true;
});

capture.addEventListener('capture', (event) => {
  const result = event.detail;
  for (const url of objectUrls.splice(0)) URL.revokeObjectURL(url);
  const url = URL.createObjectURL(result.blob);
  objectUrls.push(url);
  ui.resultImage.src = url;
  ui.resultImage.hidden = false;
  ui.resultMeta.textContent = `${result.mode}, ${result.width}x${result.height}, ${(result.blob.size / 1024).toFixed(0)} KiB, ${result.guidance.trigger} capture at ${result.capturedAt}, ready: ${String(result.guidance.ready)}`;
  ui.engine.textContent = `${result.signals.analysis.engine}${result.signals.analysis.fallbackReason ? ` (${result.signals.analysis.fallbackReason})` : ''}`;
  ui.burst.replaceChildren(
    ...result.burst.map((frame) => {
      const img = document.createElement('img');
      const frameUrl = URL.createObjectURL(frame.blob);
      objectUrls.push(frameUrl);
      img.src = frameUrl;
      img.alt = `Burst frame ${frame.offsetMs.toFixed(0)} ms`;
      return img;
    }),
  );
  const { measurements, ...summary } = result.guidance;
  ui.summary.textContent = json({ ...summary, measurements });
  ui.signals.textContent = json(result.signals);
});

capture.addEventListener('error', (event) => {
  const error = event.detail;
  ui.error.hidden = false;
  ui.error.textContent = `${error.code}: ${error.message}`;
});
