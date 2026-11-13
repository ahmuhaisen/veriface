import { FaceCaptureError } from '../errors';

/** URL that relative asset paths resolve against: the document's base URL. */
export function pageUrl(): string {
  if (typeof document !== 'undefined') return document.baseURI;
  if (typeof location !== 'undefined') return location.href;
  throw new FaceCaptureError('unsupported', 'No page URL to resolve the asset paths against');
}
