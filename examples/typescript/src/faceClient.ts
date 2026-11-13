// Server-side client for the Veriface service (contracts/openapi.yaml). Node 20+ built-ins
// only (fetch, FormData, Blob, node:http/https); no dependencies. Never import this module into
// browser code: it holds the API key.
//
// Image uploads use node:http with `Expect: 100-continue` rather than fetch. The service checks the
// key, query, profile and readiness before it reads the images, and may answer 401/400/403/404/503
// and close the connection while the upload is still in flight. fetch then fails with a bare
// "fetch failed"; waiting for 100 Continue lets the client read the problem response instead.

import { request as httpRequest, type IncomingHttpHeaders } from 'node:http';
import { request as httpsRequest } from 'node:https';

export interface FaceClientOptions {
  /** e.g. http://127.0.0.1:8080 */
  baseUrl: string;
  /** The key issued by the service operator. Read it from the environment or a secret store. */
  apiKey: string;
  /** Per attempt. Slightly above the service's request_timeout_ms (default 30 s). */
  timeoutMs?: number;
  /** Attempts for 503 overloaded / not-ready. */
  maxAttempts?: number;
}

export type Outcome = 'verified' | 'rejected' | 'review' | 'retake';
export type Decision = 'match' | 'no_match' | 'undetermined';
export type Verdict = 'accepted' | 'rejected' | 'undetermined';

export interface VerificationReason {
  source: 'probe' | 'reference' | 'risk' | 'decision';
  /** A check id, or SCORE_BELOW_THRESHOLD / NO_SCORE for source = decision. */
  code: string;
}

export interface ProfileRef {
  name: string;
  version: string;
  sha256: string;
  description?: string;
}

/** The subset of VerificationResult this example uses. */
export interface VerificationResult {
  requestId: string;
  outcome: Outcome;
  decision: Decision;
  score: number | null;
  threshold: number;
  operatingPoint: { id: string; targetFmr: number };
  calibration: { id: string; status: 'provisional' | 'calibrated' };
  reasons: VerificationReason[];
  risk: { flagged: boolean };
  profile: ProfileRef;
}

/** The subset of QualityResult this example uses. */
export interface QualityResult {
  requestId: string;
  verdict: Verdict;
  /** Failed error-severity check ids, root causes first. */
  reasons: string[];
  /** Failed warning-severity check ids. */
  warnings: string[];
  profile: ProfileRef;
}

/** RFC 9457 problem from the service. */
export class FaceServiceError extends Error {
  constructor(
    readonly status: number,
    /** e.g. overloaded, not-ready, profile-forbidden, image-decode-failed */
    readonly code: string,
    readonly requestId: string,
    message: string,
  ) {
    super(message);
    this.name = 'FaceServiceError';
  }
}

export interface ImageInput {
  bytes: Uint8Array<ArrayBuffer>;
  /** As received from the browser; the service sniffs the real format from the bytes. */
  type: string;
}

export class FaceClient {
  private readonly baseUrl: string;
  private readonly apiKey: string;
  private readonly timeoutMs: number;
  private readonly maxAttempts: number;

  constructor(options: FaceClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, '');
    this.apiKey = options.apiKey;
    this.timeoutMs = options.timeoutMs ?? 35_000;
    this.maxAttempts = options.maxAttempts ?? 3;
  }

  /** POST /v1/verifications: probe = selfie, reference = ID document (or portrait). */
  verify(probe: ImageInput, reference: ImageInput, profile = 'standard'): Promise<VerificationResult> {
    const query = new URLSearchParams({ profile, referenceKind: 'auto' });
    return this.post<VerificationResult>(`/v1/verifications?${query}`, {
      probe,
      reference,
    });
  }

  /** POST /v1/quality: early feedback on a selfie, before the verification. */
  quality(image: ImageInput, profile = 'standard'): Promise<QualityResult> {
    const query = new URLSearchParams({ profile, output: 'false' });
    return this.post<QualityResult>(`/v1/quality?${query}`, { image });
  }

  /** GET /v1/profiles/{name}: thresholds for the SDK's `profile` option. */
  async profile(name = 'standard'): Promise<{ sha256: string; profile: unknown }> {
    const response = await fetch(`${this.baseUrl}/v1/profiles/${encodeURIComponent(name)}`, {
      headers: { 'X-API-Key': this.apiKey },
      signal: AbortSignal.timeout(this.timeoutMs),
    });
    if (!response.ok) throw await toError(response, '');
    return (await response.json()) as { sha256: string; profile: unknown };
  }

  private async post<T>(path: string, parts: Record<string, ImageInput>): Promise<T> {
    // One id for every attempt, so the service log ties retries together.
    const requestId = crypto.randomUUID();
    for (let attempt = 1; ; attempt++) {
      const form = new FormData();
      for (const [name, image] of Object.entries(parts)) {
        form.append(name, new Blob([image.bytes], { type: image.type || 'image/jpeg' }), `${name}.jpg`);
      }
      const response = await postMultipart(
        new URL(`${this.baseUrl}${path}`),
        { 'X-API-Key': this.apiKey, 'X-Request-Id': requestId },
        form,
        this.timeoutMs,
      );
      if (response.status >= 200 && response.status < 300) return JSON.parse(response.body) as T;

      if (response.status === 503 && attempt < this.maxAttempts) {
        const retryAfter = Number(response.headers['retry-after'] ?? '1');
        const seconds = Number.isFinite(retryAfter) && retryAfter > 0 ? retryAfter : 1;
        await sleep(seconds * 1000 + Math.random() * 500);
        continue;
      }
      throw problemError(response.status, response.body, requestId);
    }
  }
}

interface RawResponse {
  status: number;
  headers: IncomingHttpHeaders;
  body: string;
}

/** POST a FormData body with `Expect: 100-continue` (see the note at the top of this file). */
async function postMultipart(
  url: URL,
  headers: Record<string, string>,
  form: FormData,
  timeoutMs: number,
): Promise<RawResponse> {
  // Serialise the form once: Response gives the multipart bytes and the boundary.
  const encoded = new Response(form);
  const body = new Uint8Array(await encoded.arrayBuffer());
  const contentType = encoded.headers.get('content-type') ?? 'multipart/form-data';
  const send = url.protocol === 'https:' ? httpsRequest : httpRequest;

  return new Promise<RawResponse>((resolve, reject) => {
    const req = send(url, {
      method: 'POST',
      headers: {
        ...headers,
        'Content-Type': contentType,
        'Content-Length': String(body.byteLength),
        Expect: '100-continue',
      },
      timeout: timeoutMs,
    });
    req.on('continue', () => req.end(body));
    req.on('response', (res) => {
      const chunks: Buffer[] = [];
      res.on('data', (chunk: Buffer) => chunks.push(chunk));
      res.on('end', () =>
        resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks).toString('utf8') }),
      );
      res.on('error', reject);
    });
    req.on('timeout', () => req.destroy(Object.assign(new Error('face service timeout'), { name: 'TimeoutError' })));
    req.on('error', reject);
  });
}

async function toError(response: Response, requestId: string): Promise<FaceServiceError> {
  return problemError(response.status, await response.text(), requestId, response.statusText);
}

function problemError(status: number, body: string, requestId: string, fallbackTitle = ''): FaceServiceError {
  let code = 'unknown';
  let title = fallbackTitle;
  let instance = requestId;
  try {
    const problem = JSON.parse(body) as { code?: string; title?: string; instance?: string };
    code = problem.code ?? code;
    title = problem.title ?? title;
    instance = problem.instance ?? instance;
  } catch {
    // Not a problem+json body.
  }
  return new FaceServiceError(status, code, instance, title);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
