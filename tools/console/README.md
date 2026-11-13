# Veriface dev console (`tools/console`)

**Development only. Never shipped or deployed.** This is a small browser console for testing the Veriface service by hand: selfie quality, document portrait extraction, and selfie-vs-document verification.

- The browser talks only to the Vite dev server. The dev server proxies `/api/*` to the service and **injects the `X-API-Key` header itself**, playing the role of an integrating project's backend. The key never reaches browser code.
- Use only the public-domain test images in `testdata/`, or your own photos. Do not use other people's selfies or ID documents.

## 1. Start the service with a dev configuration

Prerequisites: a built `veriface` binary (`cd service; cargo build --release -p face-server`, see `service/README.md`) and Node.js 20+.

Generate a dev API key and a service config (run once; it reuses the key afterwards):

```bash
node tools/console/scripts/dev-setup.mjs            # add --rotate for a new key
```

It writes two **git-ignored** files:

| File | Content |
|---|---|
| `tools/console/.env.local` | `VERIFACE_API_KEY=<key>` and `VERIFACE_URL=http://127.0.0.1:8080` |
| `tools/console/.dev/config.toml` | Service config pointing at the repo's `models/` and `profiles/`. It stores **only `sha256(key)`**, as the service requires, and allows every profile. |

It also points `paths.ort_library` at `service/third_party/onnxruntime/...` when you have fetched ONNX Runtime (`service/scripts/fetch-onnxruntime.ps1`). The stub engine does not need it; the real engine does.

Start the service:

```powershell
service\target\release\veriface.exe run --config tools\console\.dev\config.toml
```

Use whichever target directory you built into. Wait for `engine warmed up; ready`. While the service runs the stub engine, every check is `undetermined`; the console shows a banner saying so.

## 2. Start the console

```bash
cd tools/console
npm install
npm run dev            # http://127.0.0.1:5173
```

To point at another service, set `VERIFACE_URL` in `.env.local` and restart `npm run dev`. `npm run typecheck` runs the TypeScript checks.

## 3. What each part does

**Header.** Shows service readiness, the engine version, and whether the web SDK's guided capture is available.

**Shared controls:**

| Control | What it does |
|---|---|
| Profile | From `GET /v1/profiles`. "(key default)" sends no `?profile=`. |
| Operating point | From the selected profile's `verification.operatingPoints`. Applies to Verify only. |
| Reference kind | `auto` / `document` / `portrait`. Applies to Verify only. |
| Output image | Include or omit the normalised output image. Applies to Quality and Document. |

**Image inputs** (every slot): a file upload, a **testdata sample** (any file from `testdata/images` or `testdata/documents`, served read-only, with its expected result in the label), a **webcam snapshot**, or, when `sdk-web/dist` is built, **guided capture** with `<veriface-capture>`.

**The three tabs:**

| Tab | Endpoint | What it shows |
|---|---|---|
| Quality | `POST /v1/quality` | The verdict; reasons and warnings in priority order; the checks table (status colour, value vs allowed range, severity, experimental tag; failed checks are marked at the left edge); face box and landmarks5 drawn over the input; inter-eye distance and pose; the output image; per-stage timings plus browser round trip; and the raw JSON (output image data elided). |
| Document portrait | `POST /v1/document-portraits` | The same as Quality, plus document detection and confidence, the applied orientation, and the document corners and portrait source box drawn over the input. |
| Verify | `POST /v1/verifications` | Outcome and decision; score vs threshold with the operating point and target FMR; a clear warning when `calibration.status` is `provisional`; ordered reasons; risk checks; then the probe and reference assessments, each with its overlay. |

All geometry is drawn in input-image coordinates, as the API returns it. Overlay colours: green = face box, red = landmarks5 (1 R eye, 2 L eye, 3 nose, 4 R mouth, 5 L mouth: the subject's own right and left), blue = document corners (with "TL" marking the top-left), dashed orange = portrait source box.

API errors (RFC 9457 problem+json) appear as a red banner with the problem code and request id. If the service is down, the banner says so.

## Guided capture (optional)

When `sdk-web/dist` exists (`cd sdk-web; npm run build`), the dev server serves the SDK and its runtime files:

| URL | Served from |
|---|---|
| `/sdk/*` | `sdk-web/dist` |
| `/face-capture/face-capture.worker.js` | the SDK worker |
| `/face-capture/wasm/*` | `sdk-web/node_modules/@mediapipe/tasks-vision/wasm` |
| `/face-capture/models/*` | `models/upstream/`, the lab's pinned upstream files, byte-identical to the SDK's (`python -m facelab models bom`) |

If `models/upstream/` is empty, run `python -m facelab models fetch` in `lab/`. Without the SDK, the slots offer a plain webcam snapshot instead.

## Notes

- **Read-only samples.** The dev server only serves GET/HEAD for `/samples/*`, `/sdk/*` and `/face-capture/*`, and it rejects path traversal.
- **Key handling.** The proxy removes any `X-API-Key` header the browser sends, then sets its own.
- **Local only.** The dev server binds to 127.0.0.1. Do not expose it: anyone who can reach it can use the key.
