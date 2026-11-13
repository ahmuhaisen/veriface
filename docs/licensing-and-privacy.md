# Licensing and privacy

## Third-party licence policy

| Kind | Allowed | Not allowed |
|---|---|---|
| Code dependencies | MIT, Apache-2.0, BSD-2/3, ISC, Zlib, Unicode, CC0, BSL-1.0, MPL-2.0 (flag to the lead) | GPL, AGPL, LGPL, SSPL, "non-commercial", unlicensed |
| Model weights | Apache-2.0, MIT, BSD, CC-BY (with attribution) | Non-commercial, research-only, no licence |
| Model training data | Explicitly allows commercial use, or undisclosed **with legal review** | Known non-commercial datasets (MS1M, Glint360K, WebFace, CelebA*, VGGFace2, OULU-NPU, SiW, ...) |

How the policy is enforced:
- **Rust:** `cargo-deny` checks licences on every build in CI.
- **Models:** `models/manifest.json` records each model's licence, training-data statement and review status. The service refuses models whose review status is `rejected`.
- **NOTICE file:** every release carries the attribution texts from the manifest.

## Model bill of materials

The lab generates this table from `models/manifest.json`. Do not edit it by hand.

| Model | Licence | Training data | Review |
|---|---|---|---|
| _(generated)_ | | | |

## Privacy

- Face images and face embeddings are **biometric data**: special-category data under GDPR Art. 9, and sensitive data under the UAE PDPL and similar laws.
- The service processes in memory and returns. It stores, caches and logs **no** images, embeddings or templates. Logs and metrics carry only request ids, timings, check ids and statuses.
- Embeddings are never returned by the API.
- If a future project needs template storage:
  - it must be a separate, opt-in feature;
  - templates must be protected per ISO/IEC 24745 (irreversible, unlinkable, renewable);
  - it needs a DPIA from the integrating project.
- The integrating project owns consent, retention, the legal basis and data-subject rights.
- **Lab datasets:** consent forms on record, access-controlled storage outside git, and deletion on request. Only metrics and aggregates appear in reports.
