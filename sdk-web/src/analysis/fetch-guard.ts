/**
 * Network allow-list for the analysis worker. The worker is the SDK's own global scope, so it can
 * refuse any request that does not go to a configured asset origin. This is defence in depth: the
 * pinned MediaPipe release makes no other requests, but a future upgrade could add some (MediaPipe
 * 1.0 added usage metrics sent to Google), and those would be blocked here. It is not installed on
 * the main thread, where it would affect the host page.
 */

export interface FetchScope {
  fetch: typeof fetch;
  readonly location: { readonly href: string };
}

/** True when a request URL may be fetched: same origin as an asset, or a blob:/data: URL. */
export function isAllowedRequest(url: string, allowedOrigins: readonly string[], baseUrl: string): boolean {
  let parsed: URL;
  try {
    parsed = new URL(url, baseUrl);
  } catch {
    return false;
  }
  if (parsed.protocol === 'blob:' || parsed.protocol === 'data:') return true;
  return allowedOrigins.includes(parsed.origin);
}

export function installFetchGuard(scope: FetchScope, allowedOrigins: readonly string[]): void {
  const original = scope.fetch.bind(scope);
  scope.fetch = (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    if (isAllowedRequest(url, allowedOrigins, scope.location.href)) return original(input, init);
    return Promise.reject(
      new TypeError(`@veriface/capture: request to ${url} blocked; only self-hosted asset origins are allowed`),
    );
  };
}
