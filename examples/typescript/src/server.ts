// Minimal backend for the @veriface/capture browser SDK, on plain node:http (Node 20+).
//
//   browser (SDK) --multipart--> this server --multipart + X-API-Key--> veriface service
//
// The browser never sees the API key or the service URL. It gets back only what it needs to
// act: the outcome, the next action and reason codes to localise. No score, no face geometry.
//
//   VERIFACE_API_KEY=<key> VERIFACE_URL=http://127.0.0.1:8080 PORT=3001 node dist/server.js
//
// Routes (paths are this example's choice, not part of the face service API):
//   GET  /api/face/profile   profile thresholds for the SDK's `profile` option
//   POST /api/face/selfie    multipart part `selfie`              -> early quality feedback
//   POST /api/face/verify    multipart parts `selfie`, `document` -> verification outcome

import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';

import { FaceClient, FaceServiceError, type ImageInput, type VerificationResult } from './faceClient.js';

const apiKey = process.env.VERIFACE_API_KEY;
if (!apiKey) {
  console.error('Set VERIFACE_API_KEY to the key issued by the face service operator.');
  process.exit(2);
}
const face = new FaceClient({
  baseUrl: process.env.VERIFACE_URL ?? 'http://127.0.0.1:8080',
  apiKey,
});
const profileName = process.env.VERIFACE_PROFILE ?? 'standard';
const port = Number(process.env.PORT ?? '3001');

/** Two images at the `standard` profile's 10 MiB limit, plus multipart framing. */
const MAX_BODY_BYTES = 2 * 10 * 1024 * 1024 + 64 * 1024;

type Action = 'accept' | 'retake-selfie' | 'retake-document' | 'human-review' | 'reject';

/** docs/integration-guide.md section 6. Unknown outcomes fail safe to human review. */
function toAction(result: VerificationResult): Action {
  switch (result.outcome) {
    case 'verified':
      return 'accept';
    case 'retake':
      return result.reasons[0]?.source === 'reference' ? 'retake-document' : 'retake-selfie';
    case 'rejected':
      return 'reject';
    case 'review':
    default:
      return 'human-review';
  }
}

const server = createServer((req, res) => {
  handle(req, res).catch((error: unknown) => {
    console.error('unhandled error', error instanceof Error ? error.message : error);
    if (!res.headersSent) sendJson(res, 500, { error: 'internal' });
    else res.destroy();
  });
});

async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const url = new URL(req.url ?? '/', 'http://localhost');

  // A real application authenticates the user here (session cookie, CSRF token) and checks
  // that this user may run a verification. Omitted in this example.

  if (req.method === 'GET' && url.pathname === '/api/face/profile') {
    const { profile } = await face.profile(profileName);
    sendJson(res, 200, { profile });
    return;
  }

  if (req.method === 'POST' && url.pathname === '/api/face/selfie') {
    const parts = await readImages(req, res, ['selfie']);
    if (!parts) return;
    await forward(res, async () => {
      const result = await face.quality(parts.selfie!, profileName);
      // Localise result.reasons[0] in the browser (integration guide, section 7).
      return { verdict: result.verdict, reasons: result.reasons, warnings: result.warnings };
    });
    return;
  }

  if (req.method === 'POST' && url.pathname === '/api/face/verify') {
    const parts = await readImages(req, res, ['selfie', 'document']);
    if (!parts) return;
    await forward(res, async () => {
      const result = await face.verify(parts.selfie!, parts.document!, profileName);
      // Keep an audit record in YOUR store: request id, outcome, decision, reasons, profile
      // name/version/sha256 and calibration id. Not the images, and not the score in general logs.
      console.info(
        JSON.stringify({
          event: 'face-verification',
          requestId: result.requestId,
          outcome: result.outcome,
          decision: result.decision,
          reasons: result.reasons.map((r) => `${r.source}:${r.code}`),
          profile: `${result.profile.name}@${result.profile.version}`,
          calibration: `${result.calibration.id} (${result.calibration.status})`,
        }),
      );
      const action = toAction(result);
      return {
        requestId: result.requestId,
        outcome: result.outcome,
        action,
        // Risk codes are for reviewers; the browser shows the generic review message.
        reasons: result.reasons.filter((r) => r.source !== 'risk'),
      };
    });
    return;
  }

  sendJson(res, 404, { error: 'not-found' });
}

/** Read a multipart body (size-limited) and return the named file parts, or answer an error. */
async function readImages(
  req: IncomingMessage,
  res: ServerResponse,
  names: string[],
): Promise<Record<string, ImageInput> | undefined> {
  const contentType = req.headers['content-type'] ?? '';
  if (!contentType.toLowerCase().startsWith('multipart/form-data')) {
    sendJson(res, 415, { error: 'send multipart/form-data' });
    return undefined;
  }
  const body = await readBody(req, MAX_BODY_BYTES);
  if (!body) {
    sendJson(res, 413, { error: 'payload-too-large' });
    req.resume();
    return undefined;
  }
  // Node's built-in Request parses multipart/form-data; no dependency needed.
  let form: FormData;
  try {
    form = await new Request('http://localhost/', {
      method: 'POST',
      headers: { 'content-type': contentType },
      body,
    }).formData();
  } catch {
    sendJson(res, 400, { error: 'invalid multipart body' });
    return undefined;
  }
  const parts: Record<string, ImageInput> = {};
  for (const name of names) {
    const value = form.get(name);
    if (!(value instanceof Blob) || value.size === 0) {
      sendJson(res, 400, { error: `missing image part \`${name}\`` });
      return undefined;
    }
    parts[name] = { bytes: new Uint8Array(await value.arrayBuffer()), type: value.type };
  }
  return parts;
}

/** The whole body, or undefined when it exceeds `limit`. Kept in memory only. */
async function readBody(req: IncomingMessage, limit: number): Promise<Uint8Array | undefined> {
  const declared = Number(req.headers['content-length'] ?? '0');
  if (declared > limit) return undefined;
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of req as AsyncIterable<Buffer>) {
    size += chunk.length;
    if (size > limit) return undefined;
    chunks.push(chunk);
  }
  return new Uint8Array(Buffer.concat(chunks));
}

/** Run a service call and translate service errors into browser-safe responses. */
async function forward(res: ServerResponse, call: () => Promise<unknown>): Promise<void> {
  try {
    sendJson(res, 200, await call());
  } catch (error) {
    if (error instanceof FaceServiceError) {
      console.warn(`face service ${error.status} ${error.code} (request ${error.requestId})`);
      switch (error.code) {
        case 'payload-too-large':
        case 'unsupported-media-type':
        case 'image-decode-failed':
        case 'image-too-large':
          // The person can fix this with a new capture.
          sendJson(res, 422, { error: error.code });
          return;
        case 'overloaded':
        case 'not-ready':
          sendJson(res, 503, { error: 'busy' }, { 'retry-after': '5' });
          return;
        default:
          // 400/401/403/404/500: a configuration or service problem, not the person's.
          sendJson(res, 502, { error: 'face-service-error', requestId: error.requestId });
          return;
      }
    }
    if (error instanceof Error && (error.name === 'TimeoutError' || error.name === 'AbortError')) {
      sendJson(res, 504, { error: 'timeout' });
      return;
    }
    console.error('face service unreachable', error instanceof Error ? error.message : error);
    sendJson(res, 502, { error: 'face-service-unreachable' });
  }
}

function sendJson(
  res: ServerResponse,
  status: number,
  body: unknown,
  headers: Record<string, string> = {},
): void {
  const text = JSON.stringify(body);
  res.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
    ...headers,
  });
  res.end(text);
}

server.requestTimeout = 60_000;
server.listen(port, () => {
  console.info(`face backend example on http://localhost:${port} (profile ${profileName})`);
});
