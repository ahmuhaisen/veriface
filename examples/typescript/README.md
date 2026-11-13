# Node backend example (TypeScript)

A minimal backend for the [`@veriface/capture`](../../sdk-web/README.md) browser SDK. The browser
uploads its captures here; this server forwards them to the Veriface service with the API key,
and returns only what the page needs. Plain `node:http`, Node 20+ built-ins, no runtime
dependencies. Read the [integration guide](../../docs/integration-guide.md) first.

```
browser (SDK) ──multipart──► this server ──multipart + X-API-Key──► veriface service
                 selfie,        (key stays here)     probe, reference
                 document
```

## Files

| File | What |
|---|---|
| `src/faceClient.ts` | Server-side client: `verify`, `quality`, `profile`; `Retry-After` handling; RFC 9457 errors |
| `src/server.ts` | The HTTP server and its three routes |

## Routes

These paths are this example's choice; they are not part of the face service API.

| Route | Body (from the browser) | Calls | Returns to the browser |
|---|---|---|---|
| `GET /api/face/profile` | — | `GET /v1/profiles/{name}` | `{ profile }`, for the SDK's `profile` option |
| `POST /api/face/selfie` | multipart `selfie` | `POST /v1/quality` | `{ verdict, reasons, warnings }` for early feedback |
| `POST /api/face/verify` | multipart `selfie`, `document` | `POST /v1/verifications` | `{ requestId, outcome, action, reasons }` |

`action` is one of `accept`, `retake-selfie`, `retake-document`, `human-review`, `reject`
(integration guide, section 6). The score, the face geometry and the risk codes are **not** sent to
the browser; risk codes are for reviewers.

Service errors are translated: image problems become `422 { error: <code> }` (the person can fix
them with a new capture), `overloaded` / `not-ready` become `503 { error: "busy" }`, and
configuration problems (`401`, `403`, `404`, `400`, `500`) become `502` with the service request id
for support. Images are kept in memory only.

## Browser side

```js
const selfieEl = document.querySelector('veriface-capture[mode="selfie"]');
const documentEl = document.querySelector('veriface-capture[mode="document"]');
let selfie, card;
selfieEl.addEventListener('capture', (e) => { selfie = e.detail.blob; });
documentEl.addEventListener('capture', (e) => { card = e.detail.blob; });

async function verify() {
  const form = new FormData();
  form.append('selfie', selfie, 'selfie.jpg');
  form.append('document', card, 'document.jpg');
  const response = await fetch('/api/face/verify', { method: 'POST', body: form });
  const result = await response.json(); // { outcome, action, reasons }
  // Localise result.reasons[0].code with the tables in the integration guide, section 7.
}
```

Send only `result.blob`. The SDK's optional burst frames are not used by service v1.

## Run

```bash
npm install          # typescript and @types/node (dev only)
npm run build
VERIFACE_API_KEY=<key> VERIFACE_URL=http://127.0.0.1:8080 PORT=3001 npm start
```

| Variable | Default | |
|---|---|---|
| `VERIFACE_API_KEY` | required | Issued by the service operator. Never send it to the browser |
| `VERIFACE_URL` | `http://127.0.0.1:8080` | Base URL of the face service |
| `VERIFACE_PROFILE` | `standard` | Profile for every call |
| `PORT` | `3001` | Port of this server |

Serve this API from the **same origin** as the page that hosts the SDK. The example sends no CORS
headers on purpose.

## Implementation notes

- **`Expect: 100-continue`.** Uploads to the service use `node:http` with `Expect: 100-continue`
  instead of `fetch`. The service can refuse a request (bad key, unknown profile, not ready) before
  it reads the images and close the connection; `fetch` then reports only "fetch failed".
- **Multipart parsing** uses Node's built-in `Request.formData()`, after reading the body with a
  size limit (2 × 10 MiB + 64 KiB, matching the `standard` profile).
- **One request id per call**, reused across retries, and logged in the audit line, so the service
  log and yours can be joined.
- **Retries:** only `503`, honouring `Retry-After` plus jitter, at most 3 attempts.

## Not in this example (add them in a real service)

- User authentication, session and CSRF protection on the routes.
- Rate limiting per user, and a cap on verification attempts per session (integration guide,
  section 6).
- Your audit store. The example prints the audit record to stdout as JSON.
- TLS. Serve the page and this API over HTTPS (the SDK needs a secure context).

## Build check

Type-checked and compiled on 2026-10-02 with TypeScript 6.0.3 (`strict`,
`noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`) against `@types/node` 26.6.3 (the copies
already installed for `sdk-web/`; `npm install` here was not run): no errors. The compiled server
ran on Node 26 against a local development build of the service on the repository's public-domain
fixtures: `verified`/`accept`, `retake`/`retake-document`, `415 unsupported-media-type` (a text
file sent as the selfie), `401 unauthorized` and `404 profile-not-found` were all handled as
described.
