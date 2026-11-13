# CLAUDE.md

Read [AGENTS.md](AGENTS.md) first. It covers ownership, licence policy, biometric-data rules and conventions.

Key facts:
- **Product:** a reusable, self-hosted face service. It provides quality checks, document-portrait extraction and 1:1 selfie-vs-document verification.
- **Source of truth:** `contracts/` (API, check catalogue, schemas). Architecture is in `docs/architecture.md`; decisions are in `docs/decisions/`.
- **Stack:** Rust service (`service/`), Python lab (`lab/`, internal only), TypeScript browser SDK (`sdk-web/`).
- **Windows dev machine:**
  - Rust is installed at `%USERPROFILE%\.cargo\bin`. Add it to PATH in each shell if `cargo` is not found.
  - Python 3.12 runs via `py -3.12`.
  - Node 26.
