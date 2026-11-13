# Integration guide

For project teams that add Veriface to their product. This guide explains how to call the
service, how to read its answers, and what your project is responsible for.

The contracts are the source of truth. If this guide and a contract disagree, the contract wins:

- HTTP shapes: [`contracts/openapi.yaml`](../contracts/openapi.yaml)
- Check ids, statuses, verdict, decision and outcome rules: [`contracts/checks.yaml`](../contracts/checks.yaml)
- Profile format: [`contracts/profile.schema.json`](../contracts/profile.schema.json)

Related: [operations runbook](operations-runbook.md), [threat model](threat-model.md),
[DPIA template](dpia-template.md), [versioning and upgrades](versioning-and-upgrades.md),
[accuracy and claims](accuracy-and-claims.md).

Placeholders written as **`[PLANNED: ...]`** describe deployment features that are not built yet.
They will be filled in when that work lands.

## Contents

1. [Architecture](#1-architecture)
2. [Authentication and API keys](#2-authentication-and-api-keys)
3. [Conventions shared by every endpoint](#3-conventions-shared-by-every-endpoint)
4. [Endpoints](#4-endpoints)
5. [Reading the results](#5-reading-the-results)
6. [What to do with each outcome](#6-what-to-do-with-each-outcome)
7. [Localising check codes (English and Arabic)](#7-localising-check-codes-english-and-arabic)
8. [Errors and retries](#8-errors-and-retries)
9. [Timeouts and sizing](#9-timeouts-and-sizing)
10. [Privacy obligations](#10-privacy-obligations)
11. [Provisional calibration](#11-provisional-calibration)
12. [Example clients](#12-example-clients)

## 1. Architecture

```
browser                       your backend                         veriface service
(@veriface/capture SDK)    (holds the API key)                  (stateless, CPU only)
  │  guidance + auto-capture     │                                    │
  │──── JPEG (selfie, ID) ──────►│                                    │
  │                              │── multipart + X-API-Key ──────────►│ quality checks
  │                              │                                    │ document portrait
  │                              │◄──────────── JSON result ──────────│ 1:1 verification
  │◄── your own response ────────│  (decide, audit, store per policy) │
```

Rules:

- **The browser never calls the service.** The SDK (`sdk-web/`) only guides the person and
  captures images; it makes no network requests apart from loading its own self-hosted assets.
  Your page uploads the images to *your* backend. Your backend calls the service.
- **The API key stays on your backend.** Never put it in browser code, a mobile app bundle or a
  URL.
- **The service re-checks everything.** The SDK's guidance is advice for the person capturing. A
  modified page can send any image, so your decision must come from the service's response, never
  from SDK output (`sdk-web/README.md`, "Security and privacy").
- **The service is stateless.** It keeps no images, embeddings or templates, and it does not
  remember previous requests. Any record of a verification (who, when, outcome) is kept by your
  backend.
- **The service is a decision aid.** It returns an `outcome` (a recommended action). Your project
  decides what to do with it, and owns the legal basis, consent and retention (section 10).

The service speaks plain HTTP. When traffic leaves the machine, put TLS in front of it (a reverse
proxy). **`[PLANNED: built-in TLS. Placeholder for the TLS configuration once packaging work lands.]`**

## 2. Authentication and API keys

Every `/v1/*` call needs exactly one `X-API-Key` header.

| What | Detail |
|---|---|
| Who issues keys | The operator of the service. The configuration stores only the SHA-256 of each key; see the [runbook](operations-runbook.md#7-api-keys-and-key-rotation) |
| One key per project | Each key has a project name (used in logs), a list of allowed profiles, and an optional default profile |
| Profile access | A request without `?profile=` uses the key's default profile, else `standard`. A profile the key may not use is `403 profile-forbidden`; an unknown one is `404 profile-not-found` (404 is reported before 403) |
| Missing or wrong key | `401 unauthorized`. Two `X-API-Key` headers are also `401` |
| Health endpoints | `/health/live` and `/health/ready` need no key unless the operator sets `health_requires_key = true` |
| `/metrics` | Never authenticated. It is for the operator's monitoring, not for projects |

Store the key like any other secret: an environment variable or your secret store, never in
source control. Ask the operator for a new key if you suspect it leaked; see
[key rotation](operations-runbook.md#7-api-keys-and-key-rotation).

**`[PLANNED: key hot-reload. Today a key change needs a service restart. Placeholder for the
reload procedure.]`**

**`[PLANNED: per-key rate limiting. Today the only limit is the service-wide concurrency and
queue (section 9). Placeholder for the per-key limits and their 429 behaviour, if any.]`**

## 3. Conventions shared by every endpoint

| Topic | Rule |
|---|---|
| Base URL | Set by the operator, e.g. `http://127.0.0.1:8080`. All paths below are relative to it |
| Images | `multipart/form-data` parts. JPEG, PNG and WebP, as allowed by the profile's `input.formats`. The format is detected from the file's bytes, not from the declared content type |
| Image handling | EXIF orientation is applied, then all metadata is discarded. Coordinates in responses refer to the image after orientation |
| Size limits | Per profile: `input.maxBytes` per image (10 MiB in `standard`) and `input.maxPixels` (40 MP in `standard`), checked from the header before decoding |
| A result is a 200 | A rejected selfie or a failed verification is still `200` with a JSON body. Only errors are non-200 |
| Errors | RFC 9457 `application/problem+json` with a stable `code`; see section 8 |
| Request ids | Send `X-Request-Id` (1-128 visible ASCII characters) to correlate your logs with the service's. Otherwise the service generates a UUID. It is echoed in the `X-Request-Id` response header, in `requestId`, and in error bodies as `instance` |
| Query parameters | Unknown or duplicated query parameters are `400 invalid-request` |
| Nullable fields | Always present, as `null` when there is no value |
| Text | The service returns **codes only**. Your project turns them into messages (section 7) |

> **Note on `X-Request-Id`.** The contract says an invalid `X-Request-Id` is replaced by a
> generated id and is not an error. The current service returns `400 invalid-request` instead.
> Always send a valid id (1-128 visible ASCII characters, one header) and both behaviours are the
> same for you.

## 4. Endpoints

| Method and path | Purpose | Image parts |
|---|---|---|
| `POST /v1/quality` | Check a selfie (or any face photo) against the profile | `image` |
| `POST /v1/document-portraits` | Find the portrait on an ID card or passport page and check it as a comparison reference | `document` |
| `POST /v1/verifications` | Compare a selfie (probe) with a document or portrait (reference) | `probe`, `reference` |
| `GET /v1/profiles` | Profiles this key may use | — |
| `GET /v1/profiles/{name}` | One profile's full settings and its SHA-256 | — |
| `GET /v1/info` | Service version, catalogue version, model ids, hashes and review status | — |
| `GET /health/live` | The process is up | — |
| `GET /health/ready` | Models loaded and verified, warm-up passed | — |

The response examples below were captured from a local development build on the repository's
public-domain fixtures (`testdata/`). They are **trimmed**: `...` marks removed array items or
fields. Numbers are model outputs and will differ on your images.

### 4.1 `POST /v1/quality`

Query parameters:

| Name | Type | Default | Meaning |
|---|---|---|---|
| `profile` | string | the key's default profile, else `standard` | Which thresholds to apply |
| `output` | boolean | the profile's `selfie.output.enabled` | Include a normalised image (base64) in `output` |

```bash
curl -s -H "X-API-Key: $VERIFACE_API_KEY" -H "X-Request-Id: example-quality-1" \
  -F image=@selfie.jpg "http://127.0.0.1:8080/v1/quality?profile=standard&output=false"
```

```json
{
  "requestId": "example-quality-1",
  "subject": "selfie",
  "verdict": "accepted",
  "checks": [
    { "id": "IMAGE_RESOLUTION", "group": "input", "status": "pass", "severity": "error",
      "value": 768.0, "unit": "px", "min": 480.0, "max": null },
    { "id": "EYE_MIDPOINT_HORIZONTAL", "group": "geometry", "status": "skipped", "severity": "error",
      "value": null, "unit": "ratio", "min": null, "max": null },
    { "id": "HEAD_POSE_PITCH", "group": "pose", "status": "pass", "severity": "error",
      "value": -11.03, "unit": "deg", "min": -15.0, "max": 15.0 },
    { "id": "GAZE_FRONTAL", "group": "eyes", "status": "skipped", "severity": "warning",
      "value": null, "unit": "deg", "min": null, "max": null, "experimental": true },
    { "id": "EXPRESSION_NEUTRAL", "group": "expression", "status": "fail", "severity": "warning",
      "value": 0.975, "unit": "score", "min": null, "max": 0.6 },
    { "id": "ILLUMINATION_UNIFORMITY", "group": "illumination", "status": "fail", "severity": "warning",
      "value": 0.251, "unit": "ratio", "min": 0.4, "max": null },
    "..."
  ],
  "reasons": [],
  "warnings": ["EXPRESSION_NEUTRAL", "ILLUMINATION_UNIFORMITY"],
  "face": {
    "box": { "x": 300.33, "y": 172.79, "width": 197.37, "height": 242.35 },
    "landmarks5": [[348.65, 248.62], [435.49, 246.62], [388.99, 315.0], [346.85, 332.44], [445.47, 328.56]],
    "interEyeDistancePx": 86.86,
    "pose": { "yaw": -3.88, "pitch": -11.03, "roll": -1.44 }
  },
  "output": null,
  "profile": { "name": "standard", "version": "0.1.0", "sha256": "0d84...e46a",
               "description": "Balanced thresholds for selfie capture and ID-document reference portraits." },
  "engine": { "version": "0.1.0", "catalogueVersion": 1, "models": ["..."] },
  "timings": { "totalMs": 239.98, "queueMs": 0.0, "uploadMs": 0.23, "decodeMs": 3.61, "detectMs": 6.02,
               "landmarksMs": 21.59, "measureMs": 16.1, "segmentMs": 191.91, "evaluateMs": 0.03 }
}
```

The verdict is `accepted` although two checks failed: both have `warning` severity, and warnings
never change the verdict.

With `output=true` the `output` field holds the normalised image:

```json
"output": { "mediaType": "image/jpeg", "width": 480, "height": 640, "bytes": 48213, "data": "<base64>" }
```

The `standard` profile produces a `portrait-3x4` crop for selfies (eye midpoint and head size
placed per ICAO) and a `face-crop` for document portraits. The crop is never padded with invented
pixels and never upscaled beyond the source. `output=true` with incomplete output settings in the
profile is `400 invalid-request`.

### 4.2 `POST /v1/document-portraits`

Same query parameters as `/v1/quality` (`output` defaults to `document.output.enabled`). The part
is named `document`. It accepts a photo or scan of an ID card or passport data page, or an image
that is already a tight portrait crop (then `DOCUMENT_DETECTED` is low, which is a warning only).

```bash
curl -s -H "X-API-Key: $VERIFACE_API_KEY" \
  -F document=@id-card.jpg "http://127.0.0.1:8080/v1/document-portraits?profile=standard&output=false"
```

```json
{
  "requestId": "1da6112c-d16c-4143-aa9b-47548bc325fc",
  "subject": "document",
  "verdict": "rejected",
  "checks": [
    { "id": "IMAGE_RESOLUTION", "group": "input", "status": "pass", "severity": "error",
      "value": 1200.0, "unit": "px", "min": 640.0, "max": null },
    { "id": "INTER_EYE_DISTANCE", "group": "geometry", "status": "fail", "severity": "error",
      "value": 53.56, "unit": "px", "min": 60.0, "max": null },
    { "id": "DOCUMENT_DETECTED", "group": "document", "status": "pass", "severity": "warning",
      "value": 1.0, "unit": "score", "min": 0.5, "max": null },
    { "id": "PORTRAIT_FOUND", "group": "document", "status": "pass", "severity": "error",
      "value": 1.0, "unit": "count", "min": 1.0, "max": null },
    "..."
  ],
  "reasons": ["INTER_EYE_DISTANCE"],
  "warnings": [],
  "face": { "box": { "x": 373.68, "y": 554.53, "width": 123.8, "height": 149.26 },
            "landmarks5": ["..."], "interEyeDistancePx": 53.64,
            "pose": { "yaw": -4.64, "pitch": -8.63, "roll": 2.26 } },
  "document": {
    "detected": true,
    "confidence": 1.0,
    "corners": [[228.47, 340.51], [1274.92, 262.05], [1324.59, 922.8], [278.28, 1001.43]],
    "orientationDegrees": 0
  },
  "portraitSourceBox": { "x": 358.15, "y": 553.22, "width": 143.49, "height": 143.38 },
  "output": null,
  "profile": { "name": "standard", "version": "0.1.0", "sha256": "0d84...e46a" },
  "engine": { "...": "..." },
  "timings": { "totalMs": 411.11, "...": "..." }
}
```

Field notes (from `openapi.yaml`):

- `face` geometry is in **input-image** coordinates (the uploaded document). Exception: `pose` of
  a document portrait is measured in the rectified, upright document frame, so a tilted card does
  not add roll.
- `INTER_EYE_DISTANCE` for a document is measured on the rectified portrait, so it can differ
  slightly from `face.interEyeDistancePx` (53.56 vs 53.64 above).
- `document.corners` are in document order (the card's own top-left first), or `null` when no
  outline was found. `orientationDegrees` is the clockwise rotation that makes the document
  upright.
- `portraitSourceBox` is where the selected portrait sits in the input image.

In this example the card itself is fine, but its portrait is too small for comparison
(`INTER_EYE_DISTANCE` 53.6 px against a minimum of 60): the person should capture the card closer.

### 4.3 `POST /v1/verifications`

Query parameters:

| Name | Values | Default | Meaning |
|---|---|---|---|
| `profile` | string | key default, else `standard` | Thresholds, required checks, operating points |
| `referenceKind` | `auto`, `document`, `portrait` | `auto` | `document`: extract the portrait from a document image. `portrait`: the reference is already a face portrait (rectification is skipped). `auto`: try document extraction, fall back to portrait |
| `operatingPoint` | an operating point id of the profile | `verification.defaultOperatingPoint` | Which threshold to apply. Unknown ids are `400 invalid-request` |

Parts: `probe` (the selfie) and `reference` (the document or portrait).

```bash
curl -s -H "X-API-Key: $VERIFACE_API_KEY" -H "X-Request-Id: example-verify-1" \
  -F probe=@selfie.jpg -F reference=@id-card.jpg \
  "http://127.0.0.1:8080/v1/verifications?profile=standard&referenceKind=auto"
```

A `verified` result. It was captured with `fixtures`, a development-only profile that is the
`standard` profile with the inter-eye minimum lowered to 35 px for the small public-domain
fixture faces. It is never shipped.

```json
{
  "requestId": "example-verify-1",
  "outcome": "verified",
  "decision": "match",
  "score": 0.8918,
  "threshold": 0.45,
  "operatingPoint": { "id": "fmr-1e-4", "targetFmr": 0.0001 },
  "calibration": { "id": "provisional-2026-10-01", "status": "provisional" },
  "probe": {
    "verdict": "accepted",
    "checks": ["..."],
    "reasons": [],
    "warnings": ["MOUTH_CLOSED", "EXPRESSION_NEUTRAL"],
    "face": { "...": "..." }
  },
  "reference": {
    "verdict": "rejected",
    "checks": ["..."],
    "reasons": ["IMAGE_RESOLUTION"],
    "warnings": ["DOCUMENT_DETECTED"],
    "face": { "...": "..." },
    "kind": "portrait",
    "document": null
  },
  "risk": {
    "flagged": false,
    "checks": [
      { "id": "PROBE_REFERENCE_DUPLICATE", "group": "risk", "status": "pass", "severity": "error",
        "value": 0.306, "unit": "score", "min": null, "max": 0.5 },
      { "id": "PROBE_DOCUMENT_PRESENT", "group": "risk", "status": "pass", "severity": "error",
        "value": 0.0, "unit": "score", "min": null, "max": 0.5 },
      { "id": "PROBE_SCREEN_ARTIFACTS", "group": "risk", "status": "pass", "severity": "warning",
        "value": 0.0693, "unit": "score", "min": null, "max": 0.5, "experimental": true }
    ]
  },
  "reasons": [],
  "profile": { "name": "fixtures", "version": "0.1.0", "sha256": "0c35...0aa6" },
  "engine": { "...": "..." },
  "timings": { "totalMs": 1311.65, "queueMs": 0.0, "uploadMs": 0.34, "decodeMs": 4.34, "documentMs": 167.77,
               "detectMs": 6.49, "landmarksMs": 100.91, "measureMs": 36.62, "segmentMs": 754.48,
               "riskMs": 142.51, "embedMs": 446.01, "evaluateMs": 1.54 }
}
```

Two things this example shows:

- **`reference.verdict` is `rejected`, yet the outcome is `verified`.** The outcome depends only on
  the checks listed in the profile's `verification.requiredProbeChecks` and
  `requiredReferenceChecks`. `IMAGE_RESOLUTION` is not a required reference check in `standard`,
  so it does not block the comparison. The per-subject `verdict` is the quality-style verdict of
  `/v1/quality` and `/v1/document-portraits`; for verifications, act on `outcome`.
- With `referenceKind=auto`, the service fell back to `kind: "portrait"` because no document
  outline was found, so `reference.document` is `null`.

Other outcomes from the same fixture set (top-level fields only):

```json
{ "outcome": "rejected", "decision": "no_match", "score": 0.2592, "threshold": 0.45,
  "reasons": [ { "source": "decision", "code": "SCORE_BELOW_THRESHOLD" } ], "risk": { "flagged": false, "...": "..." } }
```

```json
{ "outcome": "review", "decision": "match", "score": 0.9840, "threshold": 0.45,
  "reasons": [ { "source": "risk", "code": "PROBE_REFERENCE_DUPLICATE" } ], "risk": { "flagged": true, "...": "..." } }
```

(The selfie was the same photo that is printed on the card, so the duplicate signal fired.)

```json
{ "outcome": "retake", "decision": "undetermined", "score": null, "threshold": 0.45,
  "reasons": [ { "source": "reference", "code": "INTER_EYE_DISTANCE" },
               { "source": "reference", "code": "SHARPNESS" } ] }
```

(A low-resolution card: required reference checks failed, so no comparison was made.)

### 4.4 `GET /v1/profiles` and `GET /v1/profiles/{name}`

```json
{ "profiles": [
  { "name": "icao-strict", "version": "0.1.0", "sha256": "6df1...d8d0",
    "description": "ICAO TR Portrait Quality limits for finished portraits; strict document reference." },
  { "name": "standard", "version": "0.1.0", "sha256": "0d84...e46a",
    "description": "Balanced thresholds for selfie capture and ID-document reference portraits." }
] }
```

`GET /v1/profiles/{name}` returns `{ "sha256": "...", "profile": { ... } }`, where `profile` follows
`contracts/profile.schema.json`. Typical use: your backend fetches it and passes `profile` to the
SDK's `profile` option, so the browser guidance uses the same thresholds as the service.

Shipped profiles:

| Profile | Use |
|---|---|
| `standard` | Remote or assisted selfie capture plus a phone-captured ID document. Default |
| `icao-strict` | Validating a finished, ICAO-style portrait as-is (e.g. an uploaded passport-style photo) |

### 4.5 `GET /v1/info`

```json
{ "version": "0.1.0", "catalogueVersion": 1,
  "models": [
    { "id": "mp-face-detector-short", "role": "face-detector", "sha256": "5c55...ec55", "reviewStatus": "pending" },
    { "id": "auraface-v1", "role": "face-embedder", "sha256": "5f17...e4d1", "reviewStatus": "pending" },
    "..."
  ] }
```

Every result also carries the same `engine` block. Record `engine.version`,
`engine.catalogueVersion`, `profile.name`, `profile.version`, `profile.sha256` and
`calibration.id` in your audit trail, so you can tell later which engine and thresholds produced a
decision.

### 4.6 Health

| Endpoint | 200 when | Otherwise |
|---|---|---|
| `/health/live` | The process runs | — |
| `/health/ready` | Models verified, profiles loaded, warm-up passed | `503 not-ready` with `Retry-After: 5`, also while shutting down |

Image endpoints also answer `503 not-ready` until the service is ready.

## 5. Reading the results

### 5.1 Check results

Each entry of `checks` (and `risk.checks`) has:

| Field | Meaning |
|---|---|
| `id` | Check id from `checks.yaml` (33 ids, section 7) |
| `group` | `input`, `face`, `geometry`, `pose`, `eyes`, `mouth`, `expression`, `occlusion`, `illumination`, `image`, `background`, `document`, `risk` |
| `status` | `pass`: value within `[min, max]`. `fail`: outside. `undetermined`: could not be assessed (a prerequisite failed or was undetermined, or the estimator was not confident). `skipped`: disabled in the profile |
| `severity` | `error` or `warning`, from the profile (else the catalogue default) |
| `value` | One number in `unit`, or `null` |
| `min`, `max` | The profile's inclusive bounds; `null` = unbounded |
| `experimental` | Present and `true` for experimental checks (`GAZE_FRONTAL`, `EYEGLASSES`, `PROBE_SCREEN_ARTIFACTS`) |

Use `value`, `min` and `max` for diagnostics and support, not for your own pass/fail logic: the
service has already applied the profile.

### 5.2 `verdict` (quality and document-portrait results, and each verification subject)

| Verdict | Rule (`checks.yaml` `verdict`) |
|---|---|
| `rejected` | At least one enabled **error**-severity check failed |
| `undetermined` | No error-severity check failed, but at least one is undetermined |
| `accepted` | Otherwise. Warning-severity checks never change the verdict |

`reasons` lists the failed error-severity check ids; `warnings` lists the failed warning-severity
ones. Both are in catalogue priority order, **root causes first** (for example `FACE_PRESENT`
before `HEAD_POSE_YAW`). Show the first reason to the person; it is usually the one to fix first.

### 5.3 `decision` (verification only)

The pure biometric result:

| Decision | Rule |
|---|---|
| `undetermined` | A required probe or reference check did not pass, or no score could be computed |
| `match` | `score >= threshold` of the selected operating point |
| `no_match` | `score < threshold` |

`score` is the cosine similarity of the two face embeddings, in [-1, 1] and model-specific. It is
`null` when no comparison was made. **Never show it to end users, and never present it as a
percentage.** It is not a probability of identity.

### 5.4 `outcome` (verification only)

The recommended action, evaluated in this order (`checks.yaml` `verification.outcome`):

| Order | Outcome | When |
|---|---|---|
| 1 | `retake` | A required probe or reference check has status `fail` (input quality problem) |
| 2 | `rejected` | `decision = no_match` |
| 3 | `review` | `decision = undetermined`, or `decision = match` but an error-severity risk check failed |
| 4 | `verified` | `decision = match` and no error-severity risk check failed |

### 5.5 Verification `reasons`

`reasons` is an array of `{ "source": ..., "code": ... }`, most important first. The service builds
it in this order (`face-core/src/verify.rs`):

1. Required checks that **failed**, probe before reference, each in catalogue priority order
   (`source` `probe` or `reference`, `code` a check id).
2. Required checks that are **undetermined** (same sources).
3. `{ "source": "decision", "code": "NO_SCORE" }` when no score was computed and the outcome is
   not `retake`.
4. `{ "source": "decision", "code": "SCORE_BELOW_THRESHOLD" }` when `decision = no_match`.
5. Error-severity risk checks that failed (`source` `risk`).

Only *required* checks appear here. Failures of non-required checks are still listed in
`probe.reasons`, `reference.reasons` and the `warnings` arrays.

### 5.6 `risk`

`risk.flagged` is `true` when an error-severity risk check failed. The three risk checks are
**replay risk signals, not certified presentation-attack detection** (ADR 0003):

| Check | What it signals |
|---|---|
| `PROBE_REFERENCE_DUPLICATE` | The selfie looks like a copy of the document portrait rather than a new capture |
| `PROBE_DOCUMENT_PRESENT` | The selfie shows a card, document or printed photo around the face |
| `PROBE_SCREEN_ARTIFACTS` | Screen or print recapture patterns. Experimental, `warning` in `standard` |

A risk flag turns a `match` into `review`, never into `rejected`: it is a reason for a human to
look, not proof of an attack.

## 6. What to do with each outcome

| Outcome | Meaning | Recommended action | User message (see 7.3) |
|---|---|---|---|
| `verified` | Faces match at the operating point; no risk flag | Continue the flow. Record the request id, outcome, profile version and calibration id. While calibration is `provisional`, keep a human confirmation step for consequential decisions (section 11) | Success message |
| `rejected` | Faces do not match at the operating point | Do not accept automatically. Allow a small number of new attempts (new selfie, optionally new document). After the limit, route to human review or an alternative identity check. Do not tell the person "you are not this person": genuine people are sometimes rejected (false non-match) | Neutral "could not confirm" message |
| `review` | Undetermined (a required check could not be assessed, or no score), or a match with a risk flag | Route to a trained reviewer who compares the images or checks the person in person. In supervised capture, staff can confirm on the spot. Do not reveal which risk signal fired | Neutral "we need to check this" message |
| `retake` | A required check failed on the selfie or the document | Ask for a new capture. Use `reasons` to say what to fix: `source: probe` means retake the selfie, `source: reference` means retake the document. Cap the attempts and fall back to review | Localised message for the first reason |

Notes:

- `review` and `rejected` handling is a **project policy**. Decide it with your risk owner, write
  it down in your DPIA ([template](dpia-template.md)), and apply it consistently.
- Fully automated decisions with legal or similarly significant effects on a person carry extra
  obligations under GDPR Art. 22 and comparable laws: human intervention, the right to contest,
  and an explanation. A human review path for `rejected` and `review` helps meet them.
- Use `/v1/quality` and `/v1/document-portraits` *before* `/v1/verifications` when you want to give
  the person feedback early (for example right after each capture). This costs one extra call per
  image; the verification re-assesses both images anyway.

## 7. Localising check codes (English and Arabic)

The service returns codes only. Your project owns the wording, in English and Arabic at minimum.
The tables below are a **starter set**; review them with your content and legal teams, and adapt
the tone to your product.

The web SDK ships its own en/ar strings for live guidance (`sdk-web/src/i18n/en.ts`, `ar.ts`),
keyed by guidance code. Those are for the capture screen; the tables here are for service results.

### 7.1 Check ids (all 33)

Messages are written for the person capturing. "Applies to" lists the catalogue subjects:
`selfie`, `document`, `portrait` (the face on a document), `pair` (verification only).

| Check id | Applies to | English | Arabic |
|---|---|---|---|
| `IMAGE_RESOLUTION` | selfie, document | The photo resolution is too low. Use a better camera or move closer. | دقة الصورة منخفضة جدًا. استخدم كاميرا أفضل أو اقترب أكثر. |
| `COMPRESSION_QUALITY` | selfie, document | The photo is too compressed. Send the original photo without editing or resizing it. | الصورة مضغوطة بشكل كبير. أرسل الصورة الأصلية دون تعديلها أو تصغيرها. |
| `FACE_PRESENT` | selfie | No face was found. Place your face inside the frame. | لم يتم العثور على وجه. ضع وجهك داخل الإطار. |
| `SINGLE_FACE` | selfie | More than one face is visible. Make sure only you are in the photo. | يظهر أكثر من وجه. تأكد من ظهورك وحدك في الصورة. |
| `INTER_EYE_DISTANCE` | selfie, portrait | Your face is too small in the photo. Move closer to the camera. | وجهك صغير جدًا في الصورة. اقترب من الكاميرا. |
| `HEAD_IN_FRAME` | selfie | Part of your head is outside the photo. Keep your whole head inside the frame. | جزء من رأسك خارج الصورة. أبقِ رأسك بالكامل داخل الإطار. |
| `EYE_MIDPOINT_HORIZONTAL` | selfie | Your face is not centred. Move so that your face is in the middle of the photo. | وجهك ليس في منتصف الصورة. تحرّك ليكون وجهك في وسط الصورة. |
| `EYE_MIDPOINT_VERTICAL` | selfie | Your eyes are too high or too low in the photo. Adjust the camera height. | عيناك مرتفعتان أو منخفضتان جدًا في الصورة. عدّل ارتفاع الكاميرا. |
| `HEAD_WIDTH_RATIO` | selfie | Your head looks too wide or too narrow in the photo. Adjust your distance from the camera. | يبدو رأسك عريضًا جدًا أو ضيقًا جدًا في الصورة. عدّل المسافة بينك وبين الكاميرا. |
| `HEAD_HEIGHT_RATIO` | selfie | Your head looks too large or too small in the photo. Adjust your distance from the camera. | يبدو رأسك كبيرًا جدًا أو صغيرًا جدًا في الصورة. عدّل المسافة بينك وبين الكاميرا. |
| `HEAD_POSE_YAW` | selfie, portrait | Look straight at the camera. Do not turn your head to the side. | انظر مباشرة إلى الكاميرا ولا تُدِر رأسك إلى الجانب. |
| `HEAD_POSE_PITCH` | selfie, portrait | Keep your chin level. Do not tilt your head up or down. | أبقِ ذقنك مستوية ولا تُمِل رأسك إلى الأعلى أو الأسفل. |
| `HEAD_POSE_ROLL` | selfie, portrait | Keep your head upright. Do not tilt it to one side. | أبقِ رأسك مستقيمًا ولا تُمِله إلى أحد الجانبين. |
| `EYES_VISIBLE` | selfie, portrait | Your eyes are not clearly visible. Remove sunglasses and move hair away from your eyes. | عيناك غير ظاهرتين بوضوح. انزع النظارة الشمسية وأبعد الشعر عن عينيك. |
| `EYES_OPEN` | selfie, portrait | Keep your eyes open. | أبقِ عينيك مفتوحتين. |
| `GAZE_FRONTAL` | selfie | Look directly at the camera lens. | انظر مباشرة إلى عدسة الكاميرا. |
| `MOUTH_CLOSED` | selfie, portrait | Keep your mouth closed. | أبقِ فمك مغلقًا. |
| `EXPRESSION_NEUTRAL` | selfie | Keep a neutral expression. | حافظ على تعابير وجه محايدة. |
| `FACE_OCCLUSION` | selfie, portrait | Part of your face is covered. Remove masks, hands or objects from in front of your face. | جزء من وجهك مغطّى. أزل الكمامة أو اليد أو أي شيء من أمام وجهك. |
| `HEAD_COVERING` | selfie | A head covering was detected. Please follow the photo rules for head coverings. | تم اكتشاف غطاء للرأس. يُرجى اتباع إرشادات الصورة الخاصة بأغطية الرأس. |
| `EYEGLASSES` | selfie | Glasses were detected. Please remove your glasses. | تم اكتشاف نظارة. يُرجى نزع النظارة. |
| `LUMINANCE_MEAN` | selfie, portrait | The lighting on your face is too dark or too bright. Move to an evenly lit place. | الإضاءة على وجهك داكنة جدًا أو ساطعة جدًا. انتقل إلى مكان ذي إضاءة متوازنة. |
| `UNDER_EXPOSURE` | selfie, portrait | Your face is too dark. Move to a brighter place. | وجهك داكن جدًا. انتقل إلى مكان أكثر إضاءة. |
| `OVER_EXPOSURE` | selfie, portrait | There is too much light or glare on the face. Avoid direct light and reflections. | يوجد ضوء زائد أو انعكاس على الوجه. تجنّب الضوء المباشر والانعكاسات. |
| `DYNAMIC_RANGE` | selfie, portrait | The photo looks flat or washed out. Improve the lighting. | تبدو الصورة باهتة. حسّن الإضاءة. |
| `ILLUMINATION_UNIFORMITY` | selfie, portrait | The lighting on your face is uneven. Face the light and avoid shadows. | الإضاءة على وجهك غير متساوية. واجه مصدر الضوء وتجنّب الظلال. |
| `SHARPNESS` | selfie, portrait | The photo is blurry. Hold the camera steady. | الصورة غير واضحة. ثبّت الكاميرا. |
| `BACKGROUND_UNIFORMITY` | selfie | The background is too busy. Stand in front of a plain background. | الخلفية مزدحمة. قف أمام خلفية سادة. |
| `DOCUMENT_DETECTED` | document | The whole document could not be found. Place the entire card inside the frame. | تعذّر العثور على المستند بالكامل. ضع البطاقة كاملة داخل الإطار. |
| `PORTRAIT_FOUND` | document | No photo was found on the document. Capture the side of the ID that shows your photo. | لم يتم العثور على صورة شخصية في المستند. صوّر وجه الهوية الذي يحتوي على صورتك. |
| `PROBE_REFERENCE_DUPLICATE` | pair | Please take a new live selfie. | يُرجى التقاط صورة سيلفي جديدة ومباشرة. |
| `PROBE_DOCUMENT_PRESENT` | selfie (risk) | Take a photo of your face only. Do not hold up a card or a photo. | التقط صورة لوجهك فقط، ولا تحمل بطاقة أو صورة أمام الكاميرا. |
| `PROBE_SCREEN_ARTIFACTS` | selfie (risk) | Take a live selfie. Do not photograph a screen or a printed photo. | التقط صورة سيلفي مباشرة، ولا تصوّر شاشة أو صورة مطبوعة. |

Risk messages are deliberately neutral. A risk flag leads to `review`, so in most flows you show the
generic review message (7.3) and keep the specific risk code for the reviewer.

### 7.2 Document-context variants

When `source` is `reference` (or the call was `/v1/document-portraits`), the problem is with the
ID photo, not the person's face. Use these variants for the checks most often seen there:

| Check id | English (reference) | Arabic (reference) |
|---|---|---|
| `IMAGE_RESOLUTION` | The photo of the document is too low in resolution. Capture it again, closer and in focus. | دقة صورة المستند منخفضة جدًا. صوّره مرة أخرى عن قرب وبوضوح. |
| `INTER_EYE_DISTANCE` | The photo on the ID is too small. Move the ID closer to the camera. | الصورة الموجودة على الهوية صغيرة جدًا. قرّب الهوية من الكاميرا. |
| `OVER_EXPOSURE` | There is glare on the ID photo. Tilt the card slightly or avoid direct light. | يوجد انعكاس ضوئي على صورة الهوية. أمِل البطاقة قليلًا أو تجنّب الضوء المباشر. |
| `UNDER_EXPOSURE` | The ID photo is too dark. Move to a brighter place. | صورة الهوية داكنة جدًا. انتقل إلى مكان أكثر إضاءة. |
| `LUMINANCE_MEAN` | The ID photo is too dark or too bright. Adjust the lighting. | صورة الهوية داكنة جدًا أو ساطعة جدًا. عدّل الإضاءة. |
| `SHARPNESS` | The ID photo is blurry. Hold the card steady and let the camera focus. | صورة الهوية غير واضحة. ثبّت البطاقة وانتظر حتى تركّز الكاميرا. |
| `EYES_VISIBLE` | The eyes on the ID photo are not clearly visible. Avoid reflections on the card. | العينان في صورة الهوية غير ظاهرتين بوضوح. تجنّب الانعكاسات على البطاقة. |
| `FACE_OCCLUSION` | Part of the face on the ID is hidden. Keep your fingers and reflections off the photo. | جزء من الوجه في صورة الهوية مخفي. أبعد أصابعك والانعكاسات عن الصورة. |

### 7.3 Decision codes and outcomes

| Code | Use | English | Arabic |
|---|---|---|---|
| `SCORE_BELOW_THRESHOLD` | `reasons[].code`, `source: decision` | We could not confirm that your selfie matches the photo on your ID. | لم نتمكن من التأكد من تطابق صورة السيلفي مع الصورة الموجودة في هويتك. |
| `NO_SCORE` | `reasons[].code`, `source: decision` | We could not compare the photos. Please try again. | تعذّرت مقارنة الصورتين. يُرجى المحاولة مرة أخرى. |
| `verified` | `outcome` | Thank you. Your identity has been confirmed. | شكرًا لك. تم التحقق من هويتك. |
| `rejected` | `outcome` | We could not confirm your identity. Please try again, or contact us for help. | لم نتمكن من التحقق من هويتك. يُرجى المحاولة مرة أخرى أو التواصل معنا للمساعدة. |
| `review` | `outcome` | Thank you. We need to check your photos; we will let you know the result. | شكرًا لك. نحتاج إلى مراجعة صورك وسنبلغك بالنتيجة. |
| `retake` | `outcome` | Please take the photo again. (Then show the message for the first reason.) | يُرجى إعادة التقاط الصورة. (ثم اعرض رسالة السبب الأول.) |

### 7.4 Error codes (optional)

Problem codes (section 8) are for your backend. If you surface any to the person, use generic
text such as "The image could not be read. Please take a new photo." /
"تعذّرت قراءة الصورة. يُرجى التقاط صورة جديدة." for `image-decode-failed`, `image-too-large`,
`payload-too-large` and `unsupported-media-type`, and "The service is busy. Please try again in a
moment." / "الخدمة مشغولة حاليًا. يُرجى المحاولة بعد قليل." for `overloaded` and `not-ready`.

## 8. Errors and retries

Errors are RFC 9457 `application/problem+json`:

```json
{ "type": "https://ahmuhaisen.github.io/veriface/problems/profile-not-found", "title": "Profile not found",
  "status": 404, "detail": "no profile named `nope`",
  "instance": "ea62a344-90b4-4d41-a573-bbf581462a81", "code": "profile-not-found" }
```

Branch on `code`, never on `title` or `detail` (those are English developer text and may change).
`instance` is the request id: quote it when you report a problem to the operator.

| HTTP | `code` | Cause | Retry? | What to do |
|---|---|---|---|---|
| 400 | `invalid-request` | Unknown or duplicate query parameter, unknown `operatingPoint`, `output=true` with incomplete profile output settings, malformed multipart, bad `X-Request-Id` | No | Fix the request |
| 401 | `unauthorized` | Missing, duplicate or unknown `X-API-Key` | No | Configuration error: alert your team |
| 403 | `profile-forbidden` | The key may not use this profile | No | Ask the operator, or use an allowed profile |
| 404 | `profile-not-found` | No such profile | No | Fix the profile name |
| 404 | `not-found` | Unknown path | No | Fix the URL |
| 405 | `method-not-allowed` | Wrong HTTP method | No | Fix the call |
| 413 | `payload-too-large` | An image is over the profile's `maxBytes`, or the body is too large | No | Downscale or recompress the image, or ask for a new capture |
| 415 | `unsupported-media-type` | Body is not multipart, or the image format is not allowed by the profile | No | Send JPEG (or another allowed format) as multipart |
| 422 | `image-decode-failed` | The bytes are not a valid image | No | Ask for a new capture |
| 422 | `image-too-large` | Over `maxPixels`, or the decoder's allocation limit | No | Downscale before sending |
| 503 | `overloaded` | Queue full, or the request missed the server's deadline. `Retry-After: 1` | Yes | Wait `Retry-After` seconds (plus jitter) and retry, a bounded number of times |
| 503 | `not-ready` | Starting, shutting down, or a model failed to load or run. `Retry-After: 5` | Yes | Wait `Retry-After` and retry; alert the operator if it persists |
| 500 | `internal` | Unexpected engine failure (details only in the service log) | Once at most | Retry once; then fail and report the request id |

Retry guidance:

- **Every endpoint is safe to retry.** The service is stateless; repeating a call has no side
  effect other than CPU time.
- **Honour `Retry-After`.** Add random jitter (for example 0-500 ms) so that many clients do not
  retry in lockstep, and cap the attempts (for example 3) and the total time. Fall back to a
  "please try again" message after that.
- **A timeout `503 overloaded` does not stop the work.** CPU work cannot be interrupted, so the
  service keeps its processing slot until the work ends. Retrying immediately and repeatedly under
  load makes overload worse; back off.
- **Connection errors** (refused, reset) during a restart: treat like `not-ready` and retry with
  backoff.
- **Do not retry 4xx** except where your user provides a new image.
- **Early refusals and large uploads.** The service checks the key, query, profile and readiness
  *before* it reads the images. It can answer `401`, `400`, `403`, `404` or `503` while your client
  is still uploading, and close the connection. Some HTTP clients then report a broken upload
  ("error while copying content to a stream" in .NET, "fetch failed" in Node's `fetch`) instead of
  the problem response. Send `Expect: 100-continue` on image uploads so the client waits for the
  go-ahead and receives the problem response: `request.Headers.ExpectContinue = true` in .NET;
  `node:http` with an `Expect` header and the `continue` event in Node (Node's `fetch` has no
  equivalent). Both examples do this, and were checked against a local build; curl does it by
  default for large bodies.

## 9. Timeouts and sizing

### 9.1 Latency to expect

Measured on a release build, Windows 11, 16 logical cores, fixture images, one request at a time,
default settings (source: [`service/README.md`](../service/README.md#performance); the machine was
shared, so each figure is a range):

| Request | Typical server time |
|---|---|
| `/v1/quality` (819x1024 portrait, with output image) | 149-248 ms |
| `/v1/document-portraits` (1600x1200 card) | 240-459 ms |
| `/v1/verifications` genuine or impostor | 700-818 ms |
| `/v1/verifications` retake (no embedding) | 411-413 ms |

Add network and upload time. Your hardware will differ: measure on the target machine. Under load,
requests also wait in the queue (`timings.queueMs`).

### 9.2 Timeouts

- The server enforces a whole-request deadline, `request_timeout_ms` (default 30 000 ms), covering
  queueing, upload and processing. Past it, the response is `503 overloaded`.
- Set your client timeout **slightly above** the server's deadline (for example 35 s with the
  default), so the server decides and you get a clean problem response. Ask the operator for the
  configured value.
- In a user-facing flow, show progress: a verification takes about a second on an idle server.

### 9.3 Images

| Aspect | Recommendation |
|---|---|
| Format | JPEG. The SDK captures JPEG at quality 0.92 with no EXIF |
| Selfie size | The SDK default camera is 1280x720 (front). `standard` needs a shorter side of at least 480 px and an inter-eye distance of at least 60 px |
| Document size | The SDK default camera is 1920x1080 (rear). `standard` needs a shorter side of at least 640 px, and the **portrait on the card** needs an inter-eye distance of at least 60 px. Most document `retake`s are a card captured too far away |
| Upper limits | `standard`: 10 MiB per image, 40 MP. Larger images only cost upload and decode time; there is no accuracy gain from sending more than the camera's native resolution |
| Do not | Re-encode at low quality, resize below the minimums, crop the face tightly yourself, or mirror the selfie |
| Body | The service accepts at most two images at the largest profile `maxBytes`, plus 64 KiB of multipart framing |

### 9.4 Capacity

The service processes at most `max_concurrency` requests at once (default: the number of logical
CPUs) and queues up to `queue_limit` more (default 32); anything beyond gets `503 overloaded`.
On the 16-core test machine, 16 concurrent verifications sustained 4.5-4.7 requests/s with the
default settings ([`service/README.md`](../service/README.md#performance)). Agree expected peak
load with the operator; the [runbook](operations-runbook.md#9-sizing-guide) has a sizing guide.

## 10. Privacy obligations

Face images and anything derived from them are **biometric data**: special-category data under
GDPR Art. 9, and sensitive personal data under the UAE PDPL and similar laws
([`licensing-and-privacy.md`](licensing-and-privacy.md)).

What the service does:

- Processes images in memory and returns. It stores, caches and logs **no** images, embeddings or
  templates.
- Never returns embeddings. The `score` is a single similarity number, not a template.
- Logs and metrics carry request ids, timings, check ids, statuses, outcomes and the project name
  only. Never images, embeddings, scores, check values or face geometry.
- Needs no network access at runtime.

What your project owns:

| Obligation | Your responsibility |
|---|---|
| Legal basis | Identify it per jurisdiction (for special-category data under GDPR: usually explicit consent, Art. 9(2)(a), or a specific legal obligation) |
| Consent and notice | Tell the person what is processed, why, by whom, for how long, and how to object or withdraw. Collect explicit consent where that is the basis |
| Alternatives | Offer a non-biometric alternative where the law or your policy requires one |
| Retention | Decide whether you keep the images at all. If you do, set a retention period and delete on schedule. The service's response contains face geometry and, with `output=true`, a face image: treat stored responses as biometric data too |
| **Never store embeddings** | The service never returns them; do not build your own from the images. Template storage would be a separate, opt-in feature with ISO/IEC 24745 protection and its own DPIA |
| Data-subject rights | Access, rectification, erasure, objection, and human review of automated decisions (GDPR Art. 22) |
| Transfers | Keep processing in the jurisdictions your DPIA allows; the service runs on your own infrastructure |
| DPIA | Required for biometric processing at scale. Use the [DPIA template](dpia-template.md) |
| Logging in your backend | Do not log the image bytes, the base64 `output`, `face` geometry or `score` in general-purpose application logs |
| Transport | Use TLS between the browser and your backend, and between your backend and the service when it is not on the same machine |

## 11. Provisional calibration

Every verification result carries `calibration`:

```json
"calibration": { "id": "provisional-2026-10-01", "status": "provisional" }
```

| Status | Meaning |
|---|---|
| `provisional` | The thresholds have **not** been calibrated on representative data. `operatingPoint.targetFmr` is a target, not a measured fact |
| `calibrated` | The lab measured the operating point on consented, representative selfie-document pairs (including demographically matched impostors) and the report supports it ([data collection protocol](data-collection-protocol.md)) |

All shipped profiles are `provisional` today. For your project this means:

- **Do not quote** a false match rate, a false non-match rate or any accuracy number to your
  users, customers or regulators based on `targetFmr` ([accuracy and claims](accuracy-and-claims.md)).
- **Keep a human in the loop** for consequential decisions: treat `verified` as "likely the same
  person", and plan a human review path for `rejected` and `review`.
- **Expect thresholds to change.** When calibration lands, profile versions and possibly
  thresholds change. Outcomes for the same images can change too. Record `profile.version`,
  `profile.sha256` and `calibration.id` with each decision.
- **Score-type quality thresholds** (sharpness, eye openness, expression, exposure) are also
  provisional. The `standard` profile notes that strict `UNDER_EXPOSURE` values penalise darker
  skin tones. Report systematic `retake`s for particular user groups to the
  Veriface team.
- **Liveness is limited.** v1 has replay risk signals only, no certified presentation-attack
  detection ([ADR 0003](decisions/0003-liveness-scope.md)). Positioning: supervised capture, and
  low-to-medium-risk remote flows with human review. Not high-assurance remote identity proofing.
- **Model review is pending.** `engine.models[].reviewStatus` is `pending` for every model until
  legal review completes ([ADR 0002](decisions/0002-models.md)). Check it before general
  availability of your feature.

## 12. Example clients

| Example | What it shows |
|---|---|
| [`examples/csharp/`](../examples/csharp/) | .NET 10 console client: `HttpClient` multipart call to `/v1/verifications`, outcome-to-action mapping, `Retry-After` handling |
| [`examples/typescript/`](../examples/typescript/) | Node 20+ backend on `node:http` that receives the SDK's captures from the browser and forwards them to the service, keeping the key server-side |
| [`tools/postman/`](../tools/postman/) | Development-only Postman collection covering every endpoint |

## Appendix: React Native / Expo uploads (SDK 57+)

From Expo SDK 57, the global `fetch` is `expo/fetch`. It rejects React Native's legacy `FormData` file parts (`{ uri, name, type }`) on the device, before anything is sent. The failure looks like a network error ("can't reach server").

Attach `File` objects from `expo-file-system` instead: they implement `Blob`.

Remember that apps must call **your backend**, never the face service directly; the API key stays server-side.
