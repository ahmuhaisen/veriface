# ADR 0003: v1 liveness scope = replay risk signals; no certified PAD

- Status: accepted (2026-10-01)

## Context

- **Selfie-to-document matching is spoofable without liveness.** 99% of DHS RIVTD attack images matched their source at FMR 1:10,000.
- **Commercial PAD is weak too.** The median commercial PAD in DHS's 2024 test rejected 78% of attacks, against a 99% goal.
- **No open PAD model has commercially clean weights and training data.**
- **Active challenge designs are patented in the US:**
  - iProov colour-flash: US 9,075,975 and 9,479,500, until about 2033.
  - FaceTec two-distance capture: US 10,776,471 and 11,157,606, until about 2035. Upheld in June 2026.
  - Google head-gesture and gaze liveness: US 8,542,879 and 8,856,541, until 2032–33.
  - iProov US 9,621,548 (selfie + document + "not a replay" + face match) reads close to this product.
- **Browsers cannot attest the camera.** There is no web API for it, so injection attacks can only be deterred.

## Decision

- **v1 ships deterministic replay risk signals** (`PROBE_REFERENCE_DUPLICATE`, `PROBE_DOCUMENT_PRESENT`, `PROBE_SCREEN_ARTIFACTS`) plus capture-quality checks. No active challenge-response.
- **Positioning:**
  - Supervised capture (staff present).
  - Low-to-medium-risk remote flows, with human review when `outcome = review`.
  - **Not** high-assurance remote identity proofing: NIST SP 800-63A-4 IAL2 and ETSI TS 119 461 both need lab-tested PAD and injection detection.
- **Claims allowed:** "internal PAD evaluation using ISO/IEC 30107-3 metrics" only. **Never** claim certified, compliant, iBeta, FIDO or CEN/TS 18099.

## Follow-ups

1. **Patent freedom-to-operate review**, before any active liveness design and before marketing the selfie+document flow in the US. Other jurisdictions are not yet checked.
2. **Collect our own attack dataset** (prints, screens, ID-card replays from consenting staff) and train an in-house passive model.
3. **Certification path** (iBeta / FIDO / CEN TS 18099 lab), only if a project requires high assurance.
