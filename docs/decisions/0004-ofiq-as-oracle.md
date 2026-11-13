# ADR 0004: OFIQ is the reference oracle in the lab, not the runtime engine

- Status: accepted (2026-10-01)

## Context

OFIQ is the ISO/IEC 29794-5 reference implementation. It is MIT-licensed C++, maintained by eu-LISA, and well placed in NIST's quality-score track. Innovatrics also bundles it in its product.

It does not fit as our runtime engine:

| Problem | Detail |
|---|---|
| Speed and size | About 0.4 s per image on a fast desktop, about 1.4 s on one Xeon core. A 7.6 s model load and 189 MB of models that can't be switched off. |
| Weak measures | Pitch (ranked 49th of 55 on one NIST set) and under-exposure (36th of 49). |
| Missing measures | Glasses, sunglasses, gaze, motion blur, red eye, background colour. |
| Head coverings | It penalises all head coverings, including religious ones. |
| Licence | Its mandatory face-parsing model (BiSeNet) was trained on CelebAMask-HQ, which is non-commercial. |
| Churn | OFIQ 2.0 replaces its detector and landmarker by the end of 2027. |

## Decision

- **Ported formulas.** Where an ISO/IEC 29794-5 measure is deterministic (luminance, exposure, dynamic range, illumination uniformity), we implement it in Rust following the standard and OFIQ's MIT-licensed source.
- **OFIQ as oracle.** The lab runs OFIQ on the same images and reports how well our measures agree with it. Where available, OFIQ's 28 conformance images are used as fixtures.
- **Shipping.** OFIQ itself is never part of the product.
