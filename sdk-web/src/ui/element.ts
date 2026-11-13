import { createDocumentCapture } from '../capture/document';
import { createSelfieCapture } from '../capture/selfie';
import type {
  CaptureStatus,
  DocumentCaptureController,
  DocumentCaptureOptions,
  DocumentCaptureResult,
  DocumentGuidanceState,
  SelfieCaptureController,
  SelfieCaptureOptions,
  SelfieCaptureResult,
  SelfieGuidanceState,
} from '../capture/types';
import { FaceCaptureError, toFaceCaptureError, type FaceCaptureErrorCode } from '../errors';
import { resolveLocale, textDirection, translate, type Locale, type MessageKey, type Messages } from '../i18n';
import { cardGuide, DEFAULT_GUIDE_FILL } from '../measure/card';
import { ellipsePath, roundedRectPath, selfieOval, shadePath, videoRectToElement } from './layout';
import { STYLES } from './styles';

/** Options of the element: every capture option except `video`, which the element provides. */
export type FaceCaptureElementOptions = Omit<SelfieCaptureOptions, 'video'> & Omit<DocumentCaptureOptions, 'video'>;

export type FaceCaptureElementMode = 'selfie' | 'document';

type ViewState = 'idle' | 'starting' | 'adjust' | 'hold' | 'ready' | 'captured' | 'error';

const TEMPLATE = /* html */ `
<div class="root" part="root" data-state="idle" data-mode="selfie">
  <div class="stage" part="stage" role="group">
    <video part="video" playsinline muted></video>
    <img class="still" part="still" alt="" hidden>
    <svg class="overlay" part="overlay" aria-hidden="true" focusable="false">
      <path class="shade" fill-rule="evenodd" d=""></path>
      <path class="frame" d=""></path>
      <path class="progress" pathLength="100" stroke-dasharray="0 100" d=""></path>
    </svg>
  </div>
  <p class="hint" part="hint" role="status" aria-live="polite" aria-atomic="true"></p>
  <p class="error" part="error" role="alert" hidden></p>
  <div class="actions" part="actions">
    <button type="button" class="capture" part="capture-button" hidden></button>
  </div>
</div>`;

/**
 * `<veriface-capture mode="selfie|document">`: camera preview with an oval (selfie) or ID-card
 * (document) overlay, live localised hints in an aria-live region, and a manual capture button.
 *
 * Attributes: `mode`, `lang` (en | ar; inherited from ancestors), `dir` (defaults from the language),
 * `autostart`, `assets-base-url`, `hide-capture-button`.
 * Properties: `options`, `messages`. Methods: `start()`, `stop()`, `capture()`, `retake()`.
 * Events (bubbling, composed; payload in `detail`): `guidance`, `capture`, `error`, `statuschange`.
 */
// eslint-disable-next-line @typescript-eslint/no-unsafe-declaration-merging -- merged with the typed event-map interface below
export class VerifaceCaptureElement extends HTMLElement {
  static readonly observedAttributes = ['mode', 'lang', 'dir', 'assets-base-url', 'hide-capture-button'];

  /** Capture options. Changes take effect at the next `start()` after a mode change or `stop()`. */
  options: Partial<FaceCaptureElementOptions> = {};
  /** Message overrides, by key (see the i18n module). */
  messages: Partial<Messages> = {};

  private readonly root: HTMLElement;
  private readonly stage: HTMLElement;
  private readonly video: HTMLVideoElement;
  private readonly still: HTMLImageElement;
  private readonly svg: SVGSVGElement;
  private readonly shade: SVGPathElement;
  private readonly frame: SVGPathElement;
  private readonly progressRing: SVGPathElement;
  private readonly hint: HTMLElement;
  private readonly errorText: HTMLElement;
  private readonly button: HTMLButtonElement;
  private controller: SelfieCaptureController | DocumentCaptureController | null = null;
  private unsubscribe: (() => void)[] = [];
  private resizeObserver: ResizeObserver | null = null;
  private stillUrl: string | null = null;
  private view: ViewState = 'idle';
  private hintKey: MessageKey | null = null;
  private errorCode: FaceCaptureErrorCode | null = null;
  private progress = 0;

  constructor() {
    super();
    const shadow = this.attachShadow({ mode: 'open' });
    shadow.innerHTML = `<style>${STYLES}</style>${TEMPLATE}`;
    this.root = required(shadow.querySelector<HTMLElement>('.root'));
    this.stage = required(shadow.querySelector<HTMLElement>('.stage'));
    this.video = required(shadow.querySelector('video'));
    this.still = required(shadow.querySelector<HTMLImageElement>('.still'));
    this.svg = required(shadow.querySelector('svg'));
    this.shade = required(shadow.querySelector<SVGPathElement>('.shade'));
    this.frame = required(shadow.querySelector<SVGPathElement>('.frame'));
    this.progressRing = required(shadow.querySelector<SVGPathElement>('.progress'));
    this.hint = required(shadow.querySelector<HTMLElement>('.hint'));
    this.errorText = required(shadow.querySelector<HTMLElement>('.error'));
    this.button = required(shadow.querySelector('button'));
    this.button.addEventListener('click', () => {
      this.onButton();
    });
    this.video.addEventListener('resize', () => {
      this.layout();
    });
    this.video.addEventListener('loadedmetadata', () => {
      this.layout();
    });
  }

  get mode(): FaceCaptureElementMode {
    return this.getAttribute('mode') === 'document' ? 'document' : 'selfie';
  }

  set mode(value: FaceCaptureElementMode) {
    this.setAttribute('mode', value);
  }

  get status(): CaptureStatus {
    return this.controller?.status ?? 'idle';
  }

  get guidance(): SelfieGuidanceState | DocumentGuidanceState | null {
    return this.controller?.guidance ?? null;
  }

  connectedCallback(): void {
    this.resizeObserver ??= new ResizeObserver(() => {
      this.layout();
    });
    this.resizeObserver.observe(this.stage);
    this.render();
    this.layout();
    // Errors are reported through the 'error' event.
    if (this.hasAttribute('autostart')) this.start().catch(() => undefined);
  }

  disconnectedCallback(): void {
    this.resizeObserver?.disconnect();
    this.disposeController();
    this.clearStill();
  }

  attributeChangedCallback(name: string, oldValue: string | null, newValue: string | null): void {
    if (oldValue === newValue) return;
    if (name === 'mode' || name === 'assets-base-url') {
      this.disposeController();
      this.clearStill();
      this.setView('idle');
      this.hintKey = null;
    }
    this.render();
    this.layout();
  }

  /** Starts the camera and guidance (the browser may ask for camera permission). */
  async start(): Promise<void> {
    this.clearStill();
    this.errorCode = null;
    let controller: SelfieCaptureController | DocumentCaptureController;
    try {
      controller = this.ensureController();
    } catch (error) {
      const failure = toFaceCaptureError(error);
      this.onError(failure);
      throw failure;
    }
    this.render();
    await controller.start();
  }

  /** Releases the camera. */
  stop(): void {
    this.controller?.stop();
  }

  /** Captures now (see CaptureController.capture). */
  capture(): Promise<SelfieCaptureResult | DocumentCaptureResult> {
    if (this.controller === null) {
      return Promise.reject(new FaceCaptureError('not-running', 'Start the capture before capturing'));
    }
    return this.controller.capture();
  }

  /** Discards the shown capture and starts again. */
  retake(): Promise<void> {
    return this.start();
  }

  private ensureController(): SelfieCaptureController | DocumentCaptureController {
    if (this.controller !== null) return this.controller;
    const baseUrl = this.getAttribute('assets-base-url');
    const options = {
      ...this.options,
      assets: { ...this.options.assets, ...(baseUrl ? { baseUrl } : {}) },
      video: this.video,
    };
    if (this.mode === 'document') {
      const controller = createDocumentCapture(options);
      this.unsubscribe = [
        controller.on('guidance', (state) => {
          this.onGuidance(state);
        }),
        controller.on('capture', (result) => {
          this.onCapture(result);
        }),
        controller.on('error', (error) => {
          this.onError(error);
        }),
        controller.on('status', (status) => {
          this.onStatus(status);
        }),
      ];
      this.controller = controller;
    } else {
      const controller = createSelfieCapture(options);
      this.unsubscribe = [
        controller.on('guidance', (state) => {
          this.onGuidance(state);
        }),
        controller.on('capture', (result) => {
          this.onCapture(result);
        }),
        controller.on('error', (error) => {
          this.onError(error);
        }),
        controller.on('status', (status) => {
          this.onStatus(status);
        }),
      ];
      this.controller = controller;
    }
    return this.controller;
  }

  private disposeController(): void {
    for (const off of this.unsubscribe) off();
    this.unsubscribe = [];
    this.controller?.dispose();
    this.controller = null;
  }

  private onGuidance(state: SelfieGuidanceState | DocumentGuidanceState): void {
    this.progress = state.stability.progress;
    if (state.code === 'READY') this.setView('ready');
    else if (state.code === 'HOLD_STILL') this.setView('hold');
    else this.setView('adjust');
    this.setHint(state.messageKey);
    this.updateProgress();
    this.dispatch('guidance', state);
  }

  private onCapture(result: SelfieCaptureResult | DocumentCaptureResult): void {
    this.showStill(result.blob);
    this.setView('captured');
    this.setHint('ui.captured');
    this.dispatch('capture', result);
  }

  private onError(error: FaceCaptureError): void {
    this.errorCode = error.code;
    this.setView('error');
    this.setHint(null);
    this.render();
    this.dispatch('error', error);
  }

  private onStatus(status: CaptureStatus): void {
    if (status === 'starting') {
      this.setView('starting');
      this.setHint('ui.starting');
    } else if (status === 'stopped' || status === 'idle') {
      this.setView('idle');
      this.setHint(null);
    }
    this.render();
    this.layout();
    this.dispatch('statuschange', status);
  }

  private onButton(): void {
    const status = this.status;
    if (status === 'running') {
      this.capture().catch(() => undefined);
    } else if (status === 'captured' || status === 'error' || status === 'stopped') {
      this.start().catch(() => undefined);
    }
  }

  private setView(view: ViewState): void {
    this.view = view;
    this.root.dataset.state = view;
  }

  private setHint(key: MessageKey | null): void {
    if (key === this.hintKey) return; // keeps the aria-live region quiet between changes
    this.hintKey = key;
    this.hint.textContent = key === null ? '' : this.t(key);
  }

  private t(key: MessageKey): string {
    return translate(key, this.locale(), this.messages);
  }

  private locale(): Locale {
    const lang = this.closest('[lang]')?.getAttribute('lang') ?? document.documentElement.lang;
    return resolveLocale(lang);
  }

  private render(): void {
    const locale = this.locale();
    this.root.lang = locale;
    this.root.dir = this.getAttribute('dir') ?? textDirection(locale);
    this.root.dataset.mode = this.mode;
    this.stage.setAttribute('aria-label', this.t(this.mode === 'document' ? 'ui.documentFrame' : 'ui.selfieFrame'));
    this.video.setAttribute('aria-label', this.t('ui.cameraPreview'));
    this.hint.textContent = this.hintKey === null ? '' : this.t(this.hintKey);
    this.errorText.hidden = this.errorCode === null;
    this.errorText.textContent = this.errorCode === null ? '' : this.t(`error.${this.errorCode}`);
    const status = this.status;
    const showCapture = status === 'running';
    const showRetake = status === 'captured' || status === 'error' || status === 'stopped';
    this.button.hidden = this.hasAttribute('hide-capture-button') || !(showCapture || showRetake);
    this.button.textContent = this.t(showCapture ? 'ui.capture' : 'ui.retake');
  }

  /** Draws the oval or card overlay to match the stage's current size. */
  private layout(): void {
    const width = this.stage.clientWidth;
    const height = this.stage.clientHeight;
    if (width === 0 || height === 0) return;
    this.svg.setAttribute('viewBox', `0 0 ${width} ${height}`);
    let shape: string;
    if (this.mode === 'selfie') {
      shape = ellipsePath(selfieOval({ width, height }));
    } else {
      const videoWidth = this.video.videoWidth || 1920;
      const videoHeight = this.video.videoHeight || 1080;
      if (this.video.videoWidth > 0) this.stage.style.aspectRatio = `${videoWidth} / ${videoHeight}`;
      // The same guide the analysis searches (an invalid guideFill is rejected by start()).
      const requested = this.options.guideFill;
      const fill = requested !== undefined && requested > 0 && requested <= 1 ? requested : DEFAULT_GUIDE_FILL;
      const guide = cardGuide(videoWidth, videoHeight, fill);
      const r = videoRectToElement(
        guide,
        { width: videoWidth, height: videoHeight },
        { width, height },
        'contain',
        this.controller?.mirrored ?? false,
      );
      shape = roundedRectPath(r, Math.min(r.width, r.height) * 0.06);
    }
    this.shade.setAttribute('d', shadePath({ width, height }, shape));
    this.frame.setAttribute('d', shape);
    this.progressRing.setAttribute('d', shape);
    this.updateProgress();
  }

  private updateProgress(): void {
    const active = this.view === 'hold' || this.view === 'ready';
    const length = active ? Math.round(Math.min(1, Math.max(0, this.progress)) * 100) : 0;
    this.progressRing.setAttribute('stroke-dasharray', `${length} 100`);
    // A zero-length dash with round caps would still paint a dot.
    this.progressRing.style.visibility = length > 0 ? 'visible' : 'hidden';
  }

  private showStill(blob: Blob): void {
    this.clearStill();
    this.stillUrl = URL.createObjectURL(blob);
    this.still.src = this.stillUrl;
    this.still.hidden = false;
  }

  private clearStill(): void {
    if (this.stillUrl !== null) URL.revokeObjectURL(this.stillUrl);
    this.stillUrl = null;
    this.still.removeAttribute('src');
    this.still.hidden = true;
  }

  private dispatch(type: string, detail: unknown): void {
    this.dispatchEvent(new CustomEvent(type, { detail, bubbles: true, composed: true }));
  }
}

function required<T>(element: T | null): T {
  if (element === null) throw new Error('veriface-capture: template element missing');
  return element;
}

export const DEFAULT_TAG_NAME = 'veriface-capture';

/** Registers the element (idempotent). A custom tag name registers a subclass. */
export function defineFaceCaptureElement(tagName: string = DEFAULT_TAG_NAME): void {
  if (typeof customElements === 'undefined' || customElements.get(tagName) !== undefined) return;
  customElements.define(
    tagName,
    tagName === DEFAULT_TAG_NAME ? VerifaceCaptureElement : class extends VerifaceCaptureElement {},
  );
}

/** Events of `<veriface-capture>`; payloads are in `event.detail`. */
export type VerifaceCaptureEventMap = Omit<HTMLElementEventMap, 'error'> & {
  guidance: CustomEvent<SelfieGuidanceState | DocumentGuidanceState>;
  capture: CustomEvent<SelfieCaptureResult | DocumentCaptureResult>;
  error: CustomEvent<FaceCaptureError>;
  statuschange: CustomEvent<CaptureStatus>;
};

// Typed listeners: `element.addEventListener('capture', (e) => e.detail.blob)`.
// eslint-disable-next-line @typescript-eslint/no-unsafe-declaration-merging -- standard custom-element event typing
export interface VerifaceCaptureElement {
  addEventListener<K extends keyof VerifaceCaptureEventMap>(
    type: K,
    listener: (this: VerifaceCaptureElement, event: VerifaceCaptureEventMap[K]) => unknown,
    options?: boolean | AddEventListenerOptions,
  ): void;
  addEventListener(
    type: string,
    listener: EventListenerOrEventListenerObject,
    options?: boolean | AddEventListenerOptions,
  ): void;
  removeEventListener<K extends keyof VerifaceCaptureEventMap>(
    type: K,
    listener: (this: VerifaceCaptureElement, event: VerifaceCaptureEventMap[K]) => unknown,
    options?: boolean | EventListenerOptions,
  ): void;
  removeEventListener(
    type: string,
    listener: EventListenerOrEventListenerObject,
    options?: boolean | EventListenerOptions,
  ): void;
}

declare global {
  interface HTMLElementTagNameMap {
    'veriface-capture': VerifaceCaptureElement;
  }
}
