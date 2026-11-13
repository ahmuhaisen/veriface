# Evaluation data collection protocol

We need consented, real data before any threshold can move from `provisional` to `calibrated`, and before we can publish any accuracy claim. This document says what to collect. The lab harness (`python -m facelab eval run`, format in `lab/src/facelab/eval/dataset.py`) consumes it.

## Consent and storage

- Get written consent per participant, covering biometric processing for evaluation, storage period and withdrawal. The dataset manifest must reference the consent record; the harness refuses real data without it.
- Store everything outside the repository, in access-controlled storage. Reports contain only aggregates and pseudonymous ids.
- Demographic attributes (sex, age band, skin tone) are **self-reported by the participant**. Never infer them, and never record them for anyone who has not consented.

## What to capture per participant

| Item | Detail |
|---|---|
| Selfie probes | 2–3 captures through the real capture flow (web SDK), on different devices (phone front camera, laptop webcam) and in different lighting |
| Document reference | A photo of the participant's own ID card or passport page, taken with the real flow. Also a flat scan if available |
| Camera elevation | Camera height relative to eye level, in degrees, for each selfie. **Vary it deliberately** (about −10° to +10°). Without this, the pitch model bias can't be separated from camera height |
| Frontal flag | Head level (Frankfort plane horizontal), looking at a fixation mark at **eye height, not at the lens**. If participants look at the lens they turn towards the camera, and the camera-elevation regression has nothing to separate. Record camera elevation relative to eye level and vary it |
| Quality labels (subset) | For about 300 selfies, a human pass/fail label per check (pose, eyes, occlusion, lighting, sharpness), from **two independent annotators**. Both labels go in the manifest (`labels`, `labelsAnnotator2`). Confusion matrices use only agreed labels; disagreements form an "ambiguous" pool. Inter-annotator agreement (Cohen's kappa) is reported per check |
| Attack samples (separate consent) | Prints, screens, and the participant's own ID card held up as a "selfie". Used for the replay risk signals and a future in-house PAD model |

## How much data

The harness only marks a threshold claimable when the exact 95% upper bound supports it. With zero errors, the "rule of 3" sets the minimum number of comparisons:

| Claim | Comparisons needed (zero errors) |
|---|---|
| FMR ≤ 1e-3 | about 3,000 impostor comparisons |
| FMR ≤ 1e-4 (default operating point) | about 30,000 **demographically matched** impostor comparisons |
| FNMR ≤ 1% | about 300 genuine comparisons |

**These counts assume independent comparisons.** Matched impostor comparisons share subjects (each person appears in about 40 comparisons per cell), so the effective sample is smaller and a "3/N" claim is optimistic even with zero errors.
- Size impostor claims on one probe and one reference per subject.
- Always check the subject-level bootstrap interval that the harness reports next to the claimable threshold.
- Extra selfies per participant still help FNMR and quality coverage, but add little impostor evidence.

**Rough sizing.** About 1,000 participants, each with a probe and a reference, give roughly 40,000 matched impostor comparisons when spread across about 24 demographic cells. That is enough for a 1e-4 claim and comparable to the DHS RIVR scale (about 1,600 volunteers). 500 participants support 1e-3 claims and an indicative 1e-4.

**Diversity.** Cover sex, age bands, skin tones, head coverings (including religious ones), glasses, beards, and old ID photos (an age gap of 5 or more years).

## Output

The harness writes a report with:
- empirical and claimable thresholds per target FMR, with subject-level bootstrap confidence intervals
- EER and the DET curve
- demographic differentials (ISO/IEC 19795-10 style)
- confusion matrices for the quality checks
- the pitch offset estimate
- proposed profile values

The lead applies these to `profiles/` and sets `calibration.status: calibrated` only when the report marks the operating point as supported.
