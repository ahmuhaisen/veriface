# Veriface

A self-hosted face service for any project. It does three things:

1. **Quality** checks whether a face photo is good enough to use, against ISO/IEC 29794-5 and ICAO portrait rules.
2. **Document portrait** finds the portrait on a photo or scan of an ID card or passport page and checks whether it can serve as a comparison reference.
3. **Verification (1:1)** compares a live selfie with that document portrait. It returns a calibrated decision and flags likely replay attacks, for example a photo of the ID card held up to the camera.

The service keeps no state: no images, embeddings or templates are stored. It runs on CPU and needs no network access at runtime. It ships as a single executable that installs as a Windows service or runs in a Linux container.

> **Scope of claims.** Liveness / presentation-attack detection is limited to *replay risk signals* in v1.
> We do not claim ISO/IEC 30107-3, iBeta, FIDO or CEN/TS 18099 certification.
> See [docs/accuracy-and-claims.md](docs/accuracy-and-claims.md).

## Repository layout

| Path | What | Language |
|---|---|---|
| `contracts/` | **Source of truth.** API (OpenAPI 3.1), check catalogue, profile and model-manifest schemas | YAML / JSON Schema |
| `profiles/` | Shipped threshold profiles (`standard`, `icao-strict`) | YAML |
| `service/` | The product: HTTP service and inference pipeline | Rust |
| `lab/` | Internal only, never shipped. Model conversion, accuracy evaluation, threshold calibration, parity tests | Python |
| `sdk-web/` | Browser capture SDK: live guidance, auto-capture of selfie and document | TypeScript |
| `models/` | Model files (git-ignored) plus `manifest.json` (committed: sources, licences, SHA-256) | — |
| `testdata/` | Small, licence-clean test images only (public-domain or synthetic) plus golden outputs | — |
| `docs/` | Architecture, decisions (ADRs), accuracy and claims, licensing, privacy | Markdown |

## How the pieces fit

```
browser (sdk-web)  ──capture──►  project backend  ──HTTP──►  veriface service (Rust)
                                                              │  ONNX Runtime (Microsoft DLL)
                                                              │  models/ (MediaPipe, AuraFace)
                                                              │  profiles/ (thresholds)
lab (Python, internal) ──produces──► models/ + manifest, profiles (calibrated), golden test outputs
```

The browser never calls the service directly. The integrating project's backend calls it.

## Start here

- Architecture: [docs/architecture.md](docs/architecture.md)
- Team rules (humans and agents): [AGENTS.md](AGENTS.md)
- Decisions: [docs/decisions/](docs/decisions/)
