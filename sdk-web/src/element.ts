/**
 * `@veriface/capture/element`: importing this module registers `<veriface-capture>`.
 * Browser only.
 */
import { defineFaceCaptureElement } from './ui/element';

export {
  DEFAULT_TAG_NAME,
  defineFaceCaptureElement,
  VerifaceCaptureElement,
  type VerifaceCaptureEventMap,
  type FaceCaptureElementMode,
  type FaceCaptureElementOptions,
} from './ui/element';

defineFaceCaptureElement();
