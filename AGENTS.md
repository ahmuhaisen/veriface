# Team rules (humans and AI agents)

These rules apply to everyone working in this repository. `CLAUDE.md` points here.

## 1. Ownership

| Area | Owner | Notes |
|---|---|---|
| `contracts/`, `docs/`, root files | **Lead** (Claude session `face-lead`) | Contracts are the interface between workstreams. Propose changes to the lead; never edit them silently. |
| `profiles/` | Lead | Structure owned by the lead; calibrated threshold values are produced by the lab and applied by the lead |
| `lab/`, `testdata/`, `models/manifest.json` | **face-dev-1** (Claude session) | The lab produces models, manifest, golden outputs and calibration |
| `tools/` | **face-dev-1** | Dev-only tooling, never shipped (e.g. the local test console) |
| `service/crates/face-core`, `face-imaging/src/geom.rs`, workspace `Cargo.toml` | Lead | Contract logic. Ask the lead for changes |
| `service/crates/face-server`, `face-imaging/src/codec.rs`, `service/scripts/`, `deploy/` | **face-senior-dev** (Claude session) | Packaging, Windows/Docker/Linux deployment, server hardening, release-check script |
| `ops/`, `service/fuzz/` | Reliability agent (lead) | Dashboards, alerts, load/soak tests, fuzzing |
| `examples/` | Lead | Client examples |
| `service/crates/face-models`, `face-quality`, `face-imaging/src/{transform,stats}.rs` | Vision agent | |
| `service/crates/face-document`, `face-verify` | Verification agent | |

**Workspace dependencies.** Add new third-party crates to `[workspace.dependencies]` in `service/Cargo.toml`, and only to that section; report each addition to the lead with its licence.
| `sdk-web/` | Web SDK agent | |

Edit only your own area. If you need a change elsewhere, ask its owner, or the lead.

**Nobody commits** (the user's decision). Changes stay in the working tree until the user asks for a commit. Finish a unit of work, run its checks, then report the changed files and the check results to the lead.

## 2. Non-negotiables

1. **Licence policy.**
   - Code dependencies must be MIT, Apache-2.0, BSD-2/3, ISC, Zlib, Unicode, CC0, BSL-1.0 or MPL-2.0 (MPL must be flagged to the lead). Fonts may also be OFL-1.1 (assets only; ship the licence text).
   - **No GPL, AGPL or LGPL.**
   - **Models:** both the weights *and* the training data must allow commercial use. Every model is recorded in `models/manifest.json` with its source URL, licence, training-data statement and SHA-256.
   - Do not ship InsightFace / antelopev2 / buffalo files. That includes the non-recognition ONNX files sitting in the AuraFace repo: we use only AuraFace's recognition model.
   - No Ultralytics/YOLO (AGPL).
2. **Biometric data stays out of git.** The only faces allowed in `testdata/` are public-domain or synthetic images, each with provenance in `testdata/LICENSES.md`. Real selfies and ID documents live in access-controlled storage outside the repo, with consent.
3. **The service never stores or logs images, embeddings or templates.** Logs carry request IDs, timings, check IDs and statuses only.
4. **No network access at runtime.** Models are local files, verified against `manifest.json` SHA-256 at startup.
5. **The contracts are the truth.** Check IDs, statuses, verdict logic and response shapes come from `contracts/`. Code that disagrees with a contract is a bug. If the contract is wrong, raise it with the lead.
6. **Report test results honestly.** A test that did not run is "not run", never "passed". If Windows Code Integrity / Smart App Control blocks a freshly built binary or DLL (Event Viewer: `Microsoft-Windows-CodeIntegrity/Operational`, events 3033/3077), **do not try to bypass or disable it**. Report it to the lead.

## 3. Engineering conventions

| Area | Conventions |
|---|---|
| **Rust** (`service/`) | Stable toolchain, edition 2024. `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. Prefer pure-Rust crates, especially for image decoding (memory-safe decoders: `image` with `zune-jpeg`, `png`, `image-webp`). ONNX Runtime goes through the `ort` crate with `load-dynamic`, loading Microsoft's official `onnxruntime.dll` / `libonnxruntime.so`. |
| **Parallel builds** | Each agent uses its own target dir to avoid cargo lock contention, e.g. `CARGO_TARGET_DIR=service/target-<agent>`. `target*/` is git-ignored. |
| **Python** (`lab/`) | Python 3.12, a local venv, pinned requirements. `ruff`, `pytest`. Scripts are runnable as `python -m facelab ...`. |
| **TypeScript** (`sdk-web/`) | Strict mode, ESM, `eslint`, `vitest`. No runtime calls to the face service from the browser. |
| **Tests** | **Unit tests** in every crate and package. **Golden tests:** the lab exports reference outputs from official MediaPipe / ONNX runs into `testdata/golden/`, and the Rust pipeline must match them within the tolerances recorded there. |
| **User-facing text** | The service returns **codes only** (check IDs, problem types). Messages are localised by the integrating project (en/ar at minimum). The web SDK ships en and ar strings. |

## 4. Communication

- Report progress, blockers and contract questions to the lead.
  - Peer Claude sessions: `SendMessage` to `face-lead`.
  - Subagents: in their final report.
- Keep reports short: what changed (files), what was verified (commands and results), what is blocked.
