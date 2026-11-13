# @veriface/capture

Browser capture SDK for **Veriface**. It guides a person to take a good selfie and a good photo of
their identity document, captures automatically, and hands the images to your app. Your app uploads
them to **your own backend**, which calls the face service.

```
browser (this SDK) ──JPEG──► your backend ──HTTP + X-API-Key──► veriface service
```

The SDK **never calls the face service** and makes **no network requests** other than loading its
own self-hosted assets. The service re-checks every image itself; the SDK's guidance only helps people
send an image that will pass.

- Selfie: front camera, live guidance, automatic capture of the sharpest still frame.
- Document: rear camera, ID-1 card guide, "move closer" until the portrait is large enough, glare
  and blur detection.
- Framework-agnostic core API, plus a `<veriface-capture>` custom element.
- English and Arabic built in (right-to-left aware); every string can be overridden.

## Contents

- [Quick start: the custom element](#quick-start-the-custom-element)
- [Core API](#core-api)
- [Guidance codes](#guidance-codes)
- [Thresholds](#thresholds)
- [Capture result](#capture-result)
- [Self-hosting the assets](#self-hosting-the-assets)
- [Browser support](#browser-support)
- [Security and privacy](#security-and-privacy)
- [Accuracy and limitations](#accuracy-and-limitations)
- [How it works](#how-it-works)
- [Development](#development)

## Quick start: the custom element

```html
<script type="module">
  import '@veriface/capture/element';
</script>

<veriface-capture id="selfie" mode="selfie" lang="ar"></veriface-capture>
```

```js
const el = document.getElementById('selfie');
el.addEventListener('capture', async (event) => {
  const result = event.detail; // see "Capture result"
  const form = new FormData();
  form.append('selfie', result.blob, 'selfie.jpg');
  await fetch('/api/my-backend/selfie', { method: 'POST', body: form }); // YOUR backend
});
el.addEventListener('error', (event) => console.warn(event.detail.code));
await el.start(); // asks for camera permission
```

| | |
|---|---|
| Attributes | `mode` (`selfie` \| `document`), `lang` (`en` \| `ar`, inherited from ancestors), `dir` (defaults from the language), `autostart`, `assets-base-url`, `hide-capture-button` |
| Properties | `options` (every core option except `video`), `messages` (string overrides), `status`, `guidance` |
| Methods | `start()`, `stop()`, `capture()` (manual capture), `retake()` |
| Events | `guidance`, `capture`, `error`, `statuschange`; the payload is in `event.detail`. They bubble and cross shadow roots. TypeScript users get typed listeners. |
| Styling | CSS custom properties `--dfc-accent`, `--dfc-accent-contrast`, `--dfc-outline`, `--dfc-shade`, `--dfc-radius`, `--dfc-error`; parts `root`, `stage`, `video`, `still`, `overlay`, `hint`, `error`, `actions`, `capture-button` |

The hint is announced through an `aria-live="polite"` region, and it only changes when the advice
changes. Errors use `role="alert"`. A manual capture button is always available (for example for
people who cannot hold still), unless you hide it.

## Core API

```ts
import { createSelfieCapture, createDocumentCapture, translate } from '@veriface/capture';

const video = document.querySelector('video')!; // you render the preview
const capture = createSelfieCapture({
  video,
  assets: { baseUrl: '/face-capture/' }, // the default
  // profile: (await fetch('/api/my-backend/face-profile').then((r) => r.json())).profile,
  burstCount: 0,
});

capture.on('guidance', (state) => {
  hint.textContent = translate(state.messageKey, 'en');
  // state.code, state.issues, state.stability.progress, state.measurements ...
});
capture.on('capture', (result) => upload(result.blob));
capture.on('error', (error) => showError(translate(`error.${error.code}`, 'en')));

await capture.start();
// capture.capture()  manual capture
// capture.stop()     release the camera, keep the model loaded (call start() to resume)
// capture.dispose()  release everything
```

`createDocumentCapture(options)` has the same shape. The document controller's `guide` property is the
card rectangle, in video pixels, that the analysis searches; draw your own overlay from it if you do
not use the element.

The selfie preview is mirrored with a CSS transform on your `<video>` (`mirrorPreview` to change).
**Captured images are never mirrored.**

Main options (all optional except `video`):

| Option | Default | |
|---|---|---|
| `assets` | `/face-capture/` on the page origin | See [self-hosting](#self-hosting-the-assets) |
| `profile` | the `standard` profile | A profile object from the service (`GET /v1/profiles/{name}`, fetched by your backend) |
| `checks` | none | Per-check overrides, e.g. `{ HEAD_POSE_YAW: { min: -10, max: 10 } }` |
| `autoCapture` | `true` | Capture as soon as the person is still and every blocking check passes |
| `stability` | selfie 600 ms / 3 frames; document 800 ms / 4 frames | Still-window settings |
| `burstCount` | `0` | Up to 10 extra frames returned with the capture |
| `jpegQuality` | `0.92` | |
| `maxAnalysisFps` | `15` | |
| `keepCameraOnCapture` | `false` | Keep the camera on for an instant retake |
| `deviceId`, `videoConstraints` | front camera 1280x720 (selfie), rear camera 1920x1080 (document) | |
| `delegate` | `'CPU'` | MediaPipe delegate (`'GPU'` is faster on some devices; test it) |
| `maxInterEyeRatio` | selfie `0.35`, document off | SDK-only "too close" bound (inter-eye distance / shorter frame side) |
| `guideFill` | `0.85` | Document only: share of the frame the card guide fills |
| `interEyeDistanceMargin` | `1.15` | Document only: guide until the portrait reaches the profile inter-eye minimum × this factor (1–2) |

## Guidance codes

Every code maps to a check ID of `contracts/checks.yaml`. Issues come in the catalogue's priority
order (root causes first); `state.code` is the hint to show, smoothed over recent frames.

| Selfie code | Check | Document code | Check |
|---|---|---|---|
| `NO_FACE` | `FACE_PRESENT` | `NO_PORTRAIT` | `PORTRAIT_FOUND` |
| `MULTIPLE_FACES` | `SINGLE_FACE` | `LOW_RESOLUTION` | `IMAGE_RESOLUTION` |
| `LOW_RESOLUTION` | `IMAGE_RESOLUTION` | `TOO_FAR` | `INTER_EYE_DISTANCE` (below min × margin) |
| `HEAD_OUT_OF_FRAME` | `HEAD_IN_FRAME` | `TOO_CLOSE` | `INTER_EYE_DISTANCE` (optional SDK bound) |
| `TOO_CLOSE` | `HEAD_IN_FRAME` (head larger than the frame) or `INTER_EYE_DISTANCE` (SDK bound) | `GLARE` | `OVER_EXPOSURE` |
| `TOO_FAR` | `INTER_EYE_DISTANCE` (below min) | `TOO_DARK` | `UNDER_EXPOSURE`, `LUMINANCE_MEAN` |
| `YAW_OUT_OF_RANGE` | `HEAD_POSE_YAW` | `TOO_BRIGHT` | `LUMINANCE_MEAN` |
| `PITCH_OUT_OF_RANGE` | `HEAD_POSE_PITCH` | `BLURRY` | `SHARPNESS` |
| `ROLL_OUT_OF_RANGE` | `HEAD_POSE_ROLL` | `HOLD_STILL` | none (all blocking checks pass) |
| `EYES_CLOSED` | `EYES_OPEN` | `READY` | none (capturing) |
| `TOO_BRIGHT` | `OVER_EXPOSURE`, `LUMINANCE_MEAN` | | |
| `TOO_DARK` | `UNDER_EXPOSURE`, `LUMINANCE_MEAN` | | |
| `BLURRY` | `SHARPNESS` | | |
| `HOLD_STILL`, `READY` | none | | |

Each issue carries `checkId`, `severity`, `value`, `min`, `max` and, for pose and framing, a
`direction` in image coordinates of the un-mirrored capture (the mirrored preview shows left and
right swapped). Messages are keyed `selfie.<CODE>`, `document.<CODE>`, `error.<code>` and `ui.*`
(`src/i18n/en.ts`, `src/i18n/ar.ts`). Override any of them with the element's `messages` property or
the third argument of `translate()`.

## Thresholds

The defaults are the `selfie` and `document` sections of `profiles/standard.yaml` (v0.1.0), for the
checks the browser can estimate. A unit test fails when they drift from that file.

- Pass a whole service profile as `profile` to use its thresholds. As in the service, a check the
  profile section does not list is skipped.
- `checks` overrides individual fields on top.
- **Error-severity** failures block auto-capture and drive the hint. **Warning-severity** failures are
  reported as `advisories` and never block (in `standard`, document darkness is a warning).
- Checks the SDK cannot assess on a frame are `undetermined` and do not block; the service decides.

## Capture result

```ts
interface CaptureResult {
  mode: 'selfie' | 'document';
  blob: Blob;              // JPEG, full camera resolution, never mirrored, no EXIF
  mimeType: 'image/jpeg';
  width: number;
  height: number;
  capturedAt: string;      // ISO 8601 UTC
  guidance: {              // how the captured frame looked to the SDK
    trigger: 'auto' | 'manual';
    ready: boolean;        // taken from a still run with every blocking check passing
    issues; advisories; checks; measurements; stableMs;
    profile: { name, version }; catalogueVersion: 1;
  };
  signals: CaptureSignals; // diagnostics only, see below
  burst: { blob; width; height; offsetMs }[]; // empty unless burstCount > 0
}
```

`checks` mirrors the API's `CheckResult` (`id`, `status`, `severity`, `value`, `unit`, `min`, `max`).
The **burst** is optional (off by default) and gives a backend a few consecutive full-resolution
frames, for future replay analysis. **Veriface service v1 does not consume burst frames:** its
API has no field for them, so send only `blob` to the service. Keep or drop the burst as your project
decides.

## Self-hosting the assets

Nothing is loaded from a CDN. By default every runtime file is served from `/face-capture/` on your
page's origin:

```
/face-capture/
├─ face-capture.worker.js                 the SDK's analysis worker (dist/)
├─ face-capture.worker.licenses.txt       third-party notices (MediaPipe, Apache-2.0)
├─ wasm/
│  ├─ vision_wasm_internal.js             MediaPipe Tasks Vision runtime, from
│  ├─ vision_wasm_internal.wasm           node_modules/@mediapipe/tasks-vision/wasm/
│  ├─ vision_wasm_nosimd_internal.js      (fallback for browsers without Wasm SIMD)
│  └─ vision_wasm_nosimd_internal.wasm
└─ models/
   ├─ face_landmarker.task                selfie (3.6 MB)
   └─ blaze_face_short_range.tflite       document (225 KB)
```

1. Copy the worker and the Wasm files at build or deploy time, from the installed package versions:
   `npx veriface-capture-assets public/face-capture`. Re-run it on every SDK upgrade; a stale worker
   is detected and reported as `asset-version-mismatch`.
2. Put the two models in `models/`. They are Apache-2.0 MediaPipe models, and not on npm:

   | File | Upstream (`https://storage.googleapis.com/mediapipe-models/…`) | SHA-256 |
   |---|---|---|
   | `face_landmarker.task` | `face_landmarker/face_landmarker/float16/1/face_landmarker.task` | `64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff` |
   | `blaze_face_short_range.tflite` | `face_detector/blaze_face_short_range/float16/1/blaze_face_short_range.tflite` | `b4578f35940bf5a1a655214a1cce5cab13eba73c1297cd78e1a04c2380b0152f` |

   `node node_modules/@veriface/capture/scripts/fetch-models.mjs <dir>` downloads and verifies them
   once, for development. In production, take them from your approved model pack. The SDK checks the
   SHA-256 before use and refuses a mismatching file. Pass `assets.*ModelSha256` to pin other model
   files, or `null` to skip the check.
3. Other locations: `assets: { baseUrl, wasmBaseUrl, faceLandmarkerModelUrl, faceDetectorModelUrl, workerUrl }`.
   **The worker must be same-origin with the page**; `workerUrl: null` forces main-thread analysis.

Server configuration:

- MIME types: `.wasm` → `application/wasm`; `.task` and `.tflite` → `application/octet-stream`.
  **IIS refuses unknown extensions by default (404.3)**, so add `<mimeMap>` entries for all three.
- Cache them long, and version the URL or the folder per SDK release.
- Content Security Policy (page): `script-src 'self' 'wasm-unsafe-eval'; worker-src 'self';
  connect-src 'self'; img-src 'self' blob:` (`blob:` only for the element's captured-image preview).
  If the worker script response carries its own CSP, it needs `script-src 'self' 'wasm-unsafe-eval';
  connect-src 'self'`.

## Browser support

| | Requirement |
|---|---|
| Page | Secure context (HTTPS or `localhost`), `getUserMedia`, `createImageBitmap`, WebAssembly, WebGL |
| Target browsers | Current Chrome / Edge, Firefox, Safari 16.4+ (iOS / iPadOS 16.4+). Verified so far in desktop Chromium only, with synthetic camera streams. |
| Analysis in a Web Worker | Needs WebGL in an `OffscreenCanvas` inside workers: Chrome / Edge, Firefox 105+, Safari 17+ |
| Fallback | Elsewhere, or if the worker cannot start, analysis runs on the main thread (`signals.analysis.engine`) |
| Frame timing | `requestVideoFrameCallback` where available, else `requestAnimationFrame` |

Wasm SIMD is used where available; the `nosimd` build covers the rest.

## Security and privacy

- **The service re-checks everything.** Guidance is advice to the person capturing, not a decision.
  Nothing the browser says is trusted: a modified page can skip the guidance or send any image.
- **`signals` are deterrence and diagnostics, NOT security.** Camera label, track settings and frame
  timing come from the browser, which the person controls. Virtual cameras, injected streams and edited
  pages can make them say anything. Use them to notice oddities and support reviews, never to accept or
  reject someone. Device and group IDs are deliberately left out.
- **No telemetry.** The SDK sends nothing anywhere. MediaPipe Tasks Vision **1.0.0 and later post
  usage metrics to Google** (`odml.pa.googleapis.com`, every 60 s, no opt-out), so this package pins
  **0.10.35**, the last release without that logger. Three further safeguards:
  - a test fails if the installed MediaPipe contains the logger;
  - the build fails if any shipped JavaScript contains a network endpoint;
  - inside the worker, `fetch` is restricted to the asset origins.

  It is also verified empirically. On 2026-10-01, a 5-minute Chromium soak (`npm run soak:network`:
  240 s of selfie analysis, 2823 frames; 60 s of document analysis, 435 frames) made no request, DNS
  lookup, socket or JavaScript-level attempt outside the local server. That includes nothing to any
  googleapis or gstatic host. Unlike the Python `mediapipe` 0.10.35 wheel, which beacons to Clearcut
  about 60 s into inference, the npm Tasks Vision 0.10.35 build has no beacon. Do not upgrade MediaPipe
  without re-running the soak.
- **Images stay in memory** and go only where your code sends them. Captured JPEGs carry no EXIF or
  location data. Revoke object URLs you create.
- **Your backend holds the service API key.** Never put it in the browser.
- Face images are biometric data: your project owns consent, retention and the legal basis.

## Accuracy and limitations

- **Thresholds are provisional**, like the profile they come from.
- **`SHARPNESS` and `EYES_OPEN` are separate estimators in the SDK.** They are not the service's
  implementations. The SDK uses the profile values as *nominal* guidance thresholds only, and the
  server's checks remain authoritative: an image the SDK accepts can still fail those checks on the
  server, and the reverse.
- **Sharpness** is a contrast-normalised Laplacian on the face, scaled to a fixed inter-eye distance,
  mapped to 0–1 with a hand-chosen constant. On five public-domain portraits with synthetic blur, it
  ranked 0, 1, 2 and 4 px Gaussian blur monotonically. The 0.30 threshold fell between 2 px and 4 px
  for four of them; the largest face (167 px inter-eye distance) still passed at 4 px, because blur is
  judged relative to face size. Best-frame selection only uses the ranking.
- **Head pose** follows `checks.yaml` `definitions.pose`:
  - **Fit.** A weighted, scaled-orthographic (weak-perspective) Procrustes fit of the 3-D landmarks to
    MediaPipe's canonical face (asset `mp-canonical-face`, 33 weighted landmarks), decomposed as
    R = Rz(c) Ry(b) Rx(a) with yaw = b, pitch = -a and roll = -c. MediaPipe's perspective matrix is
    not used.
  - **Position.** Pose is measured relative to the camera-to-face line of sight, so it does not depend
    on where the face sits in the frame.
  - **Canonical data.** The 468 vertices and 33 weights ship inside the SDK
    (`src/assets/mp-canonical-face.json`, Apache-2.0). A test pins the file to the SHA-256 in
    `models/manifest.json`.
  - **Verified.** Unit tests reproduce the lab reference (`fit_weak_perspective`) on every golden
    fixture to within 0.002 degrees. The golden invariants also hold: the same face at the top and the
    bottom of a canvas, a horizontal flip, and a 15 degree rotation.
- **Exposure** follows `definitions`:
  - **Luma** is OFIQ's: linear sRGB, Y = 0.2126 R + 0.7152 G + 0.0722 B, stored as floor(Y * 255 + 0.5).
  - **Levels.** Pixels at luma 25 or below are under-exposed, and at 247 or above over-exposed. Both
    levels are on that linear scale; in sRGB grey they are about 89 and 252.
  - **Selfie region.** The face-skin region is the landmark face oval minus the eyes, eyebrows and lips
    (the contract's no-segmentation fallback).
  - **Document region.** Document portraits have no landmark mesh, so a fixed cheek-and-nose rectangle
    stands in for the face-skin region.
- **Document inter-eye distance**:
  - **Estimate.** It comes from the face detector's eye keypoints, after a zoomed re-detection on the
    portrait. On synthetic cards it was within about ±8% of the iris-based distance the service uses.
  - **Margin.** Guidance therefore asks the person to move closer until the distance reaches the
    profile minimum × `interEyeDistanceMargin` (default 1.15, so 69 px in `standard`), which keeps
    captures clear of the server's check.
- **Eye openness** uses MediaPipe's blink blendshapes. They vary by eye shape and gaze, so treat the
  0.4 minimum with care.
- **The head box** above the forehead is estimated, not segmented.
- No liveness or presentation-attack detection happens in the browser (see
  `docs/decisions/0003-liveness-scope.md`).

## How it works

- **Frames.** For each new video frame (up to `maxAnalysisFps`), the main thread takes a
  full-resolution `ImageBitmap` and **transfers** it to the worker. The worker runs MediaPipe (face
  landmarker for selfies; face detector on the card region for documents), computes pixel statistics
  on an aligned face crop in an `OffscreenCanvas`, and transfers the frame back with plain
  measurements.
- **Main thread.** It applies the thresholds (pure functions), tracks a still window, and keeps only
  the sharpest frames of that window. A sliding-window maximum with reference-counted bitmaps keeps
  memory bounded. At capture it encodes the best frame as JPEG.
- **The worker** is one self-contained classic script with MediaPipe bundled in, so MediaPipe can load
  its Wasm loader with `importScripts()`, and no bundler support is needed. It is created from the
  asset URL. If it cannot start, the same analyzer runs on the main thread from a lazily imported
  chunk.
- **Never mirrored.** Analysis frames and captures are never mirrored. MediaPipe's left and right
  blendshape labels swap on mirrored frames, which is why the preview is mirrored with CSS only.

## Development

```bash
npm install
npm run build      # ESM library + worker + type declarations, then a no-network check of dist/
npm run lint       # ESLint (type-aware) + TypeScript for src, tests, example and configs
npm test           # Vitest unit tests
npm run example:models   # one-off: download and verify the models for the example
npm run example          # build, copy assets, serve examples/basic on http://localhost:5175
npm run soak:network     # network soak in Chromium; run before any MediaPipe upgrade (below)
```

### Network soak (run before any MediaPipe upgrade)

`npm run soak:network` builds the SDK and serves the example page from a local Vite server. It then
drives the capture element in Chromium (via `playwright-core`) on a synthetic camera:
`canvas.captureStream()` fed with public-domain portraits from `../testdata`. It runs continuous
selfie analysis for 180 s and document analysis for 30 s, with auto-capture off. It fails unless
nothing leaves the dev server origin, and it checks three independent ways:

- **Chromium NetLog** (`--log-net-log`): every URL request, DNS lookup and socket connect the browser
  made.
- **Playwright** request and websocket events for the page and its workers.
- **JavaScript hooks** on `fetch`, `XMLHttpRequest`, `navigator.sendBeacon`, `WebSocket`,
  `EventSource` and `importScripts`, in the page and inside the analysis worker. This catches attempts
  that the worker's allow-list blocks before they reach the network. Any URL matching
  googleapis, gstatic, clearcut or firebase is reported separately.

Options:

- `-- --selfie 600 --document 60` sets the durations, in seconds.
- `--out <dir>` sets where the report goes. The default is a temp folder; it holds `soak-report.json`
  and `netlog.json`.
- `--headed` shows the browser.
- `--self-test` is a positive control. It injects a beacon in the page and a fetch in the worker,
  both to `play.googleapis.com.invalid`, and passes only if both are detected.

The NetLog always shows Chromium's own IPv6 reachability check. That is a UDP `connect()` to
`[2001:4860:4860::8888]:443`, made while it resolves the local server, and it sends no datagram. The
report lists it under `browserProbes`; it fails the soak only if bytes were sent on that socket.

Prerequisites: the example models (`npm run example:models`), and the Chromium build that matches
the `playwright-core` devDependency (`npx playwright-core install chromium`).

The unit tests cover the guidance logic as pure functions over landmark, blendshape and pixel-stat
inputs. They also cover thresholds and profile handling, pose sign conventions, best-frame selection
and stability, hint smoothing, en/ar key parity, asset resolution with model integrity, the worker
network allow-list, and alignment with `contracts/checks.yaml` and `profiles/standard.yaml`. They also
fail if a MediaPipe build with telemetry, or a copyleft licence, is installed.

Licences: the runtime dependency `@mediapipe/tasks-vision` is Apache-2.0, as are the two MediaPipe
models. The worker bundles MediaPipe's JavaScript, so the build writes
`face-capture.worker.licenses.txt` (attribution plus the licence text from
`third-party/Apache-2.0.txt`). Deploy it with the worker; `copy-assets` does that. Build-time tools
(including `playwright-core`, Apache-2.0, used only by the soak) are MIT, Apache-2.0, BSD or ISC, except two transitive packages that never reach `dist/`: `lightningcss`
(MPL-2.0, Vite's CSS minifier) and `minimatch` (BlueOak-1.0.0, used by ESLint).
