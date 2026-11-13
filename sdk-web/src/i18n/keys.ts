import type { FaceCaptureErrorCode } from '../errors';
import type { DocumentGuidanceCode, SelfieGuidanceCode } from '../guidance/types';

/** UI strings of the web component. */
export type UiMessageKey =
  | 'ui.capture'
  | 'ui.retake'
  | 'ui.starting'
  | 'ui.captured'
  | 'ui.cameraPreview'
  | 'ui.selfieFrame'
  | 'ui.documentFrame';

/**
 * Every message key. Guidance keys are `selfie.<code>` and `document.<code>`, error keys are
 * `error.<code>`, so a new code without a message is a compile error in every language file.
 */
export type MessageKey =
  | `selfie.${SelfieGuidanceCode}`
  | `document.${DocumentGuidanceCode}`
  | `error.${FaceCaptureErrorCode}`
  | UiMessageKey;

export type Messages = Readonly<Record<MessageKey, string>>;
