# Threat model

STRIDE-style threat model for the Veriface service, its model pack and the browser capture SDK.
It records what is mitigated **today** (in the code or the contracts) and what is **planned**.
Review it at every MINOR release and whenever a deployment option is added.

Placeholders written as **`[PLANNED: ...]`** depend on deployment work in progress (installer,
Docker, systemd, TLS, key hot-reload, rate limiting, CI). They will be filled in when it lands.

## 1. System and trust boundaries

```
 [person + device]        [project frontend]      [project backend]           [veriface host]
  camera, browser   ──►   page + @veriface/     ──► holds API key   ──HTTP──►  veriface (Rust)
  (untrusted)             capture SDK (untrusted)   decides, audits    (B3)     ONNX Runtime DLL
                    (B1)                       (B2)                            models/ + manifest
                                                                               profiles/, config
                                                                               logs, /metrics ──► monitoring
 [supply chain]: crates.io, npm, Microsoft ORT release, MediaPipe and AuraFace upstreams, lab build (B4)
```

| Boundary | Between | Trust assumption |
|---|---|---|
| B1 | Person/device and the project frontend | Nothing from the device is trusted: camera stream, page code, SDK output and `signals` can all be forged |
| B2 | Frontend and project backend | The project authenticates its own users; the backend treats uploads as untrusted |
| B3 | Project backend and the face service | Authenticated by API key. Plain HTTP today; TLS must be added in front when the hop leaves the host |
| B4 | Upstream artifacts and the release | Dependencies, the ONNX Runtime library and model files are verified by licence policy and hashes |

### Assets

| Asset | Why it matters |
|---|---|
| Face images in transit and in memory | Biometric data (GDPR Art. 9, UAE PDPL sensitive data) |
| Verification outcomes | Drive access or onboarding decisions in the integrating project |
| API keys | Grant use of the service and of specific profiles |
| Model files and profiles | Their integrity determines the correctness of every decision |
| Service availability | Integrating flows block without it |
| Logs and metrics | Must never become a store of biometric data |

## 2. Summary

| # | Threat | STRIDE | Today | Planned |
|---|---|---|---|---|
| T1 | Malicious image uploads (decoder exploits, decompression bombs) | T, D, E | Strong | Fuzzing, decoder hardening review |
| T2 | API key theft or misuse | S, E | Partial | TLS, hot-reload, rate limiting |
| T3 | Replay and presentation attacks | S | Limited by design (ADR 0003) | Attack dataset, in-house PAD |
| T4 | Model or profile tampering | T | Strong (hashes) | Signed releases |
| T5 | Denial of service | D | Partial | Per-key rate limiting, multi-instance |
| T6 | Leakage through logs, metrics and errors | I | Strong | Log rotation guidance |
| T7 | Supply chain (dependencies, ORT, MediaPipe telemetry, model licences) | T, I | Partial | CI with cargo-deny, SBOM, signing |
| T8 | Insider threats | T, I, R, E | Partial | Signed releases, separation of duties |
| T9 | Interception between backend and service | I, T | Deployment-dependent | Built-in TLS |
| T10 | Repudiation of decisions | R | Partial (ids and versions in every response) | Integrating project's audit trail |

## 3. Threats in detail

### T1. Untrusted image uploads

Images come from public web portals and phones; anyone can craft one.

| Attack | Mitigation today | Where |
|---|---|---|
| Memory-corruption exploits in image decoders (the class of libwebp CVE-2023-4863, OpenCV CVE-2025-53644, libpng CVEs) | Pure-Rust decoders only: the `image` crate with `png`, `image-webp` and `zune-jpeg`. No C image libraries are compiled in (`image` features limited to `jpeg`, `png`, `webp`) | ADR 0001; `service/Cargo.toml` |
| Unsafe code in our own crates | `unsafe_code = "deny"` for every workspace crate | `service/Cargo.toml` `[workspace.lints.rust]` |
| Format confusion (a declared `image/jpeg` that is something else) | The format is sniffed from the magic bytes, never from the declared content type, and checked against the profile's `input.formats` | `face-imaging/src/codec.rs` |
| Oversized upload | Whole-body limit (2 × the largest profile `maxBytes` + 64 KiB), `Content-Length` pre-check, and a per-part limit while the part streams in. `413 payload-too-large` | `face-server/src/api.rs` |
| Decompression bomb (small file, huge dimensions) | `maxPixels` checked from the **header before any pixel is decoded** (`422 image-too-large`), then the decoder's allocation limit (512 MiB) during decoding | `codec.rs` |
| Metadata attacks (EXIF, ICC, XMP payloads) | EXIF orientation is applied, then **all** metadata is dropped; output images are written without metadata | `codec.rs` |
| Memory exhaustion through many parallel uploads | A processing slot is taken **before** the upload is read, so at most `max_concurrency` bodies are in memory | `face-server/src/limiter.rs` |
| A panic in one request | Release builds keep unwinding; panics are caught per request and become `500 internal`; the service keeps running | `Cargo.toml` `[profile.release]`, `api.rs` |
| Long `detail` echoing input | Problem `detail` is cut at 512 characters | `problem.rs` |

Residual risks:

- **`zune-jpeg` is built with its SIMD features on** (`x86`, `neon`, enabled through the `image`
  crate's default dependency on it), so the JPEG decoder contains `unsafe` SIMD code. Without
  those features it would be `forbid(unsafe_code)`. ADR 0001's safety argument holds only
  partially for JPEG. Options: accept and fuzz, or disable the SIMD features (slower decode).
  **Decision needed from the lead.**
- `image-too-large` is the only answer to a crafted header; the decoder still parses headers of
  hostile files. Fuzzing is not in place.
- ONNX Runtime (C++) processes tensors derived from images. Inputs are fixed-size tensors built by
  our code, not raw files, which narrows this surface.

**`[PLANNED: fuzzing of face-imaging decode in CI (cargo-fuzz or equivalent). Placeholder for the
fuzz targets and schedule.]`**

### T2. API key theft or misuse

| Attack | Mitigation today | Where |
|---|---|---|
| Key leaked from the browser | Contract and SDK rule: the browser never calls the service; keys live only on project backends | `openapi.yaml`, `sdk-web/README.md`, integration guide |
| Key stolen from the service host config | The config stores only SHA-256 hashes; a stolen config does not reveal keys. Keys are random 32-byte values (the documented generation), so the unsalted hash is not brute-forceable | `auth.rs`, `service/README.md` |
| Timing attacks on key comparison | The presented key is hashed and compared with **every** configured hash in constant time | `auth.rs` |
| A stolen key used against other profiles | Each key lists its allowed profiles (`403 profile-forbidden`) | `auth.rs` |
| Ambiguous auth (two headers) | Exactly one `X-API-Key` header is required | `auth.rs` |
| Key sniffed on the network | Plain HTTP today. Mitigation is deployment: bind to `127.0.0.1` when co-located, or put TLS in front | Runbook 2.4 |
| Detecting misuse | Every request log line carries the `project` of the key used; metrics show request rates | `api.rs` |
| Revocation | Remove the hash and restart | Runbook 7.2 |

Residual risks: a stolen key works until the operator removes it and restarts; no per-key rate
limit slows abuse; no key expiry.

**`[PLANNED: built-in TLS (and optionally mTLS between backend and service).]`**
**`[PLANNED: key hot-reload, so revocation needs no restart.]`**
**`[PLANNED: per-key rate limiting.]`**

### T3. Replay and presentation attacks

The verification compares a selfie with a document portrait. Without liveness, a photo of the ID
card, a printed photo, a screen replay or an injected video stream can produce a `match`: 99% of
DHS RIVTD attack images matched their source at FMR 1:10,000 (ADR 0003).

| Attack | Mitigation today (v1) | Limit |
|---|---|---|
| Holding up the ID card (or its portrait) as the selfie | `PROBE_REFERENCE_DUPLICATE` (similarity above the genuine range, near-identical aligned crops and pose) and `PROBE_DOCUMENT_PRESENT` (card-shaped outline around the face). Error severity in `standard`: a match becomes `review` | Deterministic heuristics, provisional thresholds |
| Screen or print replay | `PROBE_SCREEN_ARTIFACTS` (moire, pixel grid). **Experimental, warning severity** | Not decisive on its own |
| Camera injection, virtual cameras, deepfakes | **None in the service.** Browsers cannot attest the camera. SDK `signals` are diagnostics only and can be forged | Out of scope for v1 |
| Masks, high-quality prints of another person | None beyond the face comparison | Out of scope for v1 |

Positioning (ADR 0003): supervised capture (staff present), and low-to-medium-risk remote flows
with human review on `review`. **Not** for high-assurance remote identity proofing (NIST SP
800-63A-4 IAL2, ETSI TS 119 461). Claims are limited to "replay risk signals" and "internal PAD
evaluation using ISO/IEC 30107-3 metrics"; never "liveness detection", "certified", iBeta, FIDO or
CEN/TS 18099.

Integrating projects must: route `review` to humans; not reveal which risk signal fired; cap
attempts per session; and prefer supervised capture where the risk warrants it.

Planned (ADR 0003 follow-ups): collect a consented attack dataset and train an in-house passive
PAD model; patent freedom-to-operate review before any active liveness design; certification only
if a project requires high assurance.

### T4. Model and profile tampering

A modified model or profile silently changes every decision (for example a lowered threshold).

| Attack | Mitigation today | Where |
|---|---|---|
| Replacing or corrupting a model file | Every model and asset file is checked against `models/manifest.json` (exact byte size and SHA-256) at startup; any mismatch stops the service. Sessions are created from exactly the bytes that were hashed | `face-models/src/manifest.rs` |
| Loading a model that failed legal review | `review.status: rejected` models are refused; `pending` is logged as a warning | `manifest.rs` |
| Replacing the ONNX Runtime library | Must expose the 1.28 C API (older libraries, such as the System32 copy, are refused); version 1.28.x enforced when `VERSION_NUMBER` is present; optional exact SHA-256 pin (`engine.onnxruntime_sha256`) | `face-models/src/runtime.rs` |
| Editing a profile | Profiles are validated at startup; each response reports `profile.sha256`, so a modified file is visible in every result and audit record | `profiles.rs`, `dto.rs` |
| Swapping a profile written for another model | A profile's `verification.model` must be the loaded embedder | `pipeline.rs` |
| Tampered release package in transit | `SHA256SUMS` over every packaged file | `service/scripts/package.ps1` |

Residual risk: an attacker who can write to the install directory can edit **both** a model file
and `manifest.json` (and `SHA256SUMS`). The manifest is integrity, not authenticity. Protect the
install directory with file-system permissions (the service account needs read access only), and
compare `/v1/info` model hashes and `profile.sha256` against the release notes.

**`[PLANNED: signed release artifacts (code-signed binary, signed manifest or checksums).
Placeholder for the signing scheme and verification step.]`**

### T5. Denial of service

| Attack | Mitigation today | Where |
|---|---|---|
| Request floods | Admission control: `max_concurrency` slots, `queue_limit` waiting places, then immediate `503 overloaded` with `Retry-After: 1`. Cheap rejection before the upload is read | `limiter.rs`, `api.rs` |
| Slow requests, slow uploads | Whole-request deadline `request_timeout_ms` (queue, upload, processing); `503 overloaded` when missed | `api.rs` |
| Expensive images | Byte and pixel limits per profile, header check before decode | T1 |
| Unauthenticated work | API key checked before the body is read; health checks are cheap. `/metrics` is unauthenticated: restrict it on the network | `api.rs` |
| Retry storms | `Retry-After` on every 503; the integration guide requires jittered, bounded retries | Integration guide 8 |
| Crash loops | Windows service recovery restarts with back-off (5 s, 30 s, 120 s) | `service.rs` |

Residual risks: one key (or one unauthenticated client hitting `/health/*` or `/metrics`) can
consume the whole queue; a timed-out request keeps its CPU slot until its work ends; there is a
single instance.

**`[PLANNED: per-key rate limiting.]`** **`[PLANNED: multi-instance deployment behind a load
balancer.]`**

### T6. Leakage through logs, metrics and errors

| Risk | Mitigation today | Where |
|---|---|---|
| Images, embeddings or templates written anywhere | The service never stores, caches or logs them (AGENTS.md non-negotiable 3). Uploaded bytes are dropped before the processing slot is released | `api.rs` |
| Biometric values in logs | Per-request log fields are restricted to ids, statuses, check ids and timings. **Never** scores, check values or face geometry. Undetermined causes (debug level) are text causes, not values | `api.rs` `LogFields` |
| Biometric values in metrics | Labels carry endpoints, statuses, stages, check ids and outcomes only | `metrics.rs` |
| Embeddings in responses | Never returned. The `score` is a single similarity number | `openapi.yaml` |
| Engine internals in error responses | Engine error details go to the log only; the client gets `not-ready` or `internal` | `api.rs` |
| API keys in logs | Never logged; only the project name | `auth.rs` |
| Echoed input | Problem `detail` can echo a profile name or similar, cut at 512 characters | `problem.rs` |
| Responses themselves | Contain face geometry and, with `output=true`, a face image. The integrating project must treat stored responses as biometric data | Integration guide 10 |
| Telemetry from ONNX Runtime | ORT environment created with telemetry off | `runtime.rs` |

Residual risk: integrating projects logging full responses or images in their own systems. The
integration guide and the DPIA template call this out.

### T7. Supply chain

| Risk | Mitigation today | Planned |
|---|---|---|
| Copyleft or non-commercial code dependencies | Licence policy (no GPL/AGPL/LGPL); `service/deny.toml` allows only permissive licences and MPL-2.0, denies yanked crates and unknown registries or git sources | **`[PLANNED: cargo-deny run on every build in CI. The config exists; CI does not yet.]`** |
| Known-vulnerable crates | Exact `ort` pin (`=2.0.0-rc.13`); `Cargo.lock`; packaging builds with `--locked` | **`[PLANNED: cargo-deny advisories / cargo-audit in CI, with a policy for advisory response time.]`** |
| `ort` binding is a release candidate | Pinned exactly; the inference engine is Microsoft's official library loaded at runtime, not a bundled build | Track `ort` 2.0 final |
| ONNX Runtime library | Microsoft's official build, fetched and hash-verified by `service/scripts/fetch-onnxruntime.*`; optional runtime pin; Authenticode-signed on Windows | Pin by default in the shipped config |
| **MediaPipe telemetry (browser)** | MediaPipe Tasks Vision **1.0.0 and later post usage metrics to Google** (`odml.pa.googleapis.com`, every 60 s, no opt-out). The SDK pins **0.10.35**, the last release without that logger. A test fails if the installed MediaPipe contains the logger; the build fails if shipped JavaScript contains a network endpoint; the worker restricts `fetch` to asset origins; a 5-minute Chromium network soak (2026-10-01) saw no request outside the local server | Re-run `npm run soak:network` before any MediaPipe upgrade (`sdk-web/README.md`) |
| **MediaPipe telemetry (lab)** | The Python `mediapipe` 0.10.35 and 1.0.x wheels embed Google's Clearcut logger and opened connections to Google about 60 s into local inference. The lab pins the telemetry-free 0.10.21 and refuses to import any version not on its allow-list (`lab/README.md`). The lab is never shipped | Re-run the telemetry probe before adding a version |
| Model licences and training data | Each model's licence, training-data statement and review status are in the manifest; the lab build fails if any InsightFace file name appears; AuraFace's InsightFace side files are never fetched | Legal review of AuraFace and the MediaPipe scope statements (all `pending`) |
| Model integrity from upstream | Upstream artifacts are hash-checked (`upstream.sha256` required in the manifest); `models build` is byte-reproducible | — |
| SDK assets from a CDN | Nothing is loaded from a CDN; models are hash-checked in the browser before use | — |
| Build machine compromise | None specific today | **`[PLANNED: CI-built releases from a clean runner, SBOM, signed artifacts.]`** |

### T8. Insider threats

| Actor and risk | Mitigation today | Planned |
|---|---|---|
| Operator lowers a threshold or disables a check in a profile | `profile.sha256` and version in every response and log line; projects record them | Profile changes only through releases; change control |
| Operator replaces a model and its manifest entry | `/v1/info` hashes comparable with release notes | Signed manifest (T4) |
| Operator reads images | The service has no images at rest; capturing them would require a code change or network interception | TLS (T9) |
| Developer adds image or embedding logging | Team rule (AGENTS.md non-negotiable 3); code review; the `LogFields` design only carries ids and statuses | Automated test that log output contains no numeric check values |
| Developer weakens the decoder or licence policy | Workspace lints and `deny.toml` in the repository | CI enforcement |
| Integrating project staff misuse results (for example identification beyond 1:1) | Out of the service's scope; the API is 1:1 only, returns no embeddings, and keeps no gallery | DPIA per project, access controls in the project |
| Lab staff mishandle evaluation data | Consent records, storage outside git, aggregates only in reports ([data collection protocol](data-collection-protocol.md)) | — |

### T9. Interception between backend and service

Plain HTTP carries face images and results. Mitigation today is deployment: co-locate and bind to
`127.0.0.1`, or terminate TLS in a reverse proxy and firewall the service port. Browser to backend
is the project's HTTPS (the SDK requires a secure context).

**`[PLANNED: built-in TLS.]`**

### T10. Repudiation

Every response carries `requestId` (also in the `X-Request-Id` header and the service log),
`engine.version`, model hashes, `profile.version`, `profile.sha256` and `calibration.id`. The
service keeps no decision record by design; **the integrating project's audit trail** is the
record of who was verified, when and with what outcome.

## 4. Open items for the lead

1. `zune-jpeg` SIMD features are on, so the JPEG decoder is not `forbid(unsafe_code)` (T1).
2. CI does not exist yet, so the `cargo-deny` policy is not enforced automatically (T7).
3. No fuzzing of the decode path (T1).
4. No signing of releases or of the manifest (T4).
5. An invalid `X-Request-Id` is rejected with `400`, while the contract says it is replaced (not a
   security issue; listed for completeness).
