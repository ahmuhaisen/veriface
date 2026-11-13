# Changelog

All notable changes to Veriface. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
What each version number means is defined in [docs/versioning-and-upgrades.md](docs/versioning-and-upgrades.md).

## [0.1.0] - Unreleased

First version.

### Compatibility

| Part | Version |
|---|---|
| Binary (`veriface`) | 0.1.0 |
| API | `/v1` (`contracts/openapi.yaml` 1.0.0) |
| Check catalogue | `catalogueVersion` 1 (33 checks) |
| Profiles | `standard` 0.1.0, `icao-strict` 0.1.0. Calibration `provisional-2026-10-01`, status **provisional** |
| Model pack | `manifestVersion` 1, generated 2026-10-01 by facelab 0.1.0. `mp-face-detector-short`, `mp-face-detector-full`, `mp-face-landmarker`, `mp-face-blendshapes`, `mp-selfie-multiclass`, `auraface-v1`, asset `mp-canonical-face`. Review status of every model: **pending** |
| ONNX Runtime | 1.28.2 (Microsoft official build), loaded at runtime |
| Web SDK | `@veriface/capture` 0.1.0 (MediaPipe Tasks Vision pinned to 0.10.35) |

### Added

- HTTP service (Rust, single binary): `POST /v1/quality`, `POST /v1/document-portraits`,
  `POST /v1/verifications`, `GET /v1/profiles`, `GET /v1/profiles/{name}`, `GET /v1/info`,
  `GET /health/live`, `GET /health/ready`, `GET /metrics`.
- Check catalogue with 33 checks across input, face, geometry, pose, eyes, mouth, expression,
  occlusion, illumination, image, background, document and risk groups.
- Verification `decision` (`match`, `no_match`, `undetermined`) and `outcome` (`verified`,
  `rejected`, `review`, `retake`) with ordered `reasons`.
- Replay risk signals: `PROBE_REFERENCE_DUPLICATE`, `PROBE_DOCUMENT_PRESENT`,
  `PROBE_SCREEN_ARTIFACTS` (experimental). Not certified presentation-attack detection.
- API keys stored as SHA-256 hashes, with per-key profile access and default profile.
- RFC 9457 problem responses; `503` with `Retry-After` for overload and readiness.
- Admission control (`max_concurrency`, `queue_limit`) and a whole-request deadline
  (`request_timeout_ms`).
- Startup verification of every model file's size and SHA-256; refusal of `rejected` models.
- Windows service install/uninstall with automatic restart on failure; graceful shutdown.
- JSON logs without biometric data; Prometheus metrics.
- Browser capture SDK `@veriface/capture` with English and Arabic strings.
- Documentation: integration guide, operations runbook, versioning and upgrades, threat model,
  DPIA template; .NET and Node example clients.

### Known limitations

- Thresholds are provisional: `targetFmr` is a target, not a measured rate.
- Legal review of the models (AuraFace training data, MediaPipe model-card scope statements) is
  pending.
- Plain HTTP only; terminate TLS in front of the service.
- API key changes need a restart. No per-key rate limiting.
- An invalid `X-Request-Id` returns `400 invalid-request`; the contract says it should be replaced
  by a generated id.
