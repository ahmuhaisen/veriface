# Accuracy targets and what we may claim

## Targets for verification

Sources: NIST SP 800-63A-4 §3.11 (2025) and the DHS RIVR goals.

| Metric | Target (goal) | Minimum acceptable |
|---|---|---|
| False match rate at the default operating point | ≤ 1 in 10,000 | — |
| False non-match rate at that point | ≤ 1% | ≤ 5% |
| Failure to extract a face (probe or document) | ≤ 1% | ≤ 5% |
| Worst demographic group vs overall population | ≤ 25% worse (NIST SP 800-63A-4) | — |

**How thresholds are calibrated:**
- Use genuine and impostor selfie-vs-document pairs.
- Include **demographically matched impostors**. With them, DHS saw false matches rise about tenfold.

## Targets for quality checks

The benchmark is the best published commercial results on BioLab FVC-onGoing FICV:
- **Most checks:** ≤ 2% equal error rate. This applies to geometry, pose, eyes and mouth.
- **Hard checks:** ≤ 10%. These are expression, head covering, tinted lenses and shadows.

## How we measure

1. **Evaluation set.** Consented, in-house selfie + document pairs, plus labelled quality photos.
   - Start with 500–1,000 people.
   - Diversity: gender, age, skin tone, head coverings, glasses, phones and webcams.
   - Public selfie–document datasets are research-only, so they cannot be used.
2. **Reporting.** Every release produces an accuracy report following ISO/IEC 19795-1 and a demographic report following ISO/IEC 19795-10. A regression blocks the release.
3. **Independent testing.** NIST FRTE 1:1 accepts submissions, and NIST SP 800-63A-4 asks for periodic independent testing and published results.

## What we may and may not claim

| May claim | Must not claim |
|---|---|
| "Face comparison accuracy measured per ISO/IEC 19795-1 on <dataset description>, FNMR x% at FMR y" | Any accuracy number without the dataset and operating point |
| "Quality checks aligned with ISO/IEC 29794-5 and the ICAO Portrait Quality TR" | "ISO/IEC 29794-5 compliant / conformant" (no conformance testing done) |
| "Replay risk signals; internal PAD evaluation using ISO/IEC 30107-3 metrics" | "Liveness detection", "certified", "iBeta", "FIDO", "CEN/TS 18099" |

Responses carry `calibration.status`. While it is `provisional`, the `targetFmr` of an operating point is a target, not a measured fact.
