# ADR 0002: Licence-clean models: MediaPipe for analysis, AuraFace for matching

- Status: accepted (2026-10-01). AuraFace legal review is **pending**.

## Context

The product is closed-source and commercial. A model is usable only if its weights **and** its training data allow commercial use.

Most open face models fail on the training data:
- InsightFace (antelopev2 / buffalo) weights are non-commercial.
- AdaFace, EdgeFace, TopoFR, TransFace and LVFace are trained on MS1M, Glint360K or WebFace, whose terms forbid commercial use.
- The synthetic-data models (DigiFace-1M, IDiff-Face, DCFace) have non-commercial licences, or their generators were trained on non-commercial data.
- The open PAD models are trained on OULU-NPU, SiW or CelebA-Spoof, all non-commercial.

## Decision

| Job | Model | Licence |
|---|---|---|
| Detection, landmarks, blendshapes, segmentation | Google MediaPipe models | Apache-2.0 |
| Face embedding | AuraFace v1 (fal.ai), **its recognition model only** | Apache-2.0 |

AuraFace's Hugging Face repository also contains InsightFace files: the SCRFD detector, 2d106det, 1k3d68 and genderage. They are byte-identical to antelopev2 and therefore non-commercial, so they must **never** be shipped.

## Consequences

- **AuraFace's training data is undisclosed.** Legal review is required before general availability. Its manifest entry carries `review.status: pending`.
- **AuraFace is weaker than SOTA on pose and age gaps.** It scores 95.2 on CFP-FP against 98.9 for ArcFace-R100, and 96.1 on AgeDB against 98.4. Age gaps matter here because document photos can be years old. Fallbacks, in order:
  1. Fine-tune it on consented selfie–document pairs.
  2. Licence InsightFace's models: a weights licence, not an SDK.
- **No passive PAD model in v1.** Replay risk signals are deterministic (see ADR 0003).
- **AuraFace ships in FP32.** Lab experiment L6 (2026-10-01) tested INT8 variants. Acceptance rule: cosine to FP32 ≥ 0.995 **and** faster at the same thread count. None passed:
  - Dynamic INT8 kept accuracy (0.9971) but ran about 20× slower, because ORT CPU executes it via ConvInteger.
  - Static QDQ ran 2.5–3× faster but its best minimum cosine was only 0.9909, shifting scores by up to 0.026, which is a meaningful threshold move.
  - FP32 baseline on the dev machine (Ryzen 7 250): about 200 ms per face with 1 thread, about 70 ms with 4 threads.
  - Revisit: percentile-calibrated QDQ on a few hundred real aligned faces once consented data exists. Rerun with `facelab experiment auraface-int8`.
- **MediaPipe model-card scope statement (legal review).** The BlazeFace and multiclass-segmenter model cards say: "Any form of surveillance or identity recognition is explicitly out of scope and not enabled by this technology."
  - It is a statement of intended scope, not a licence term; the weights are Apache-2.0.
  - In this product, MediaPipe models do only detection, landmarks and segmentation, for quality checks and face alignment. The identity comparison is done by AuraFace.
  - Counsel to confirm. The manifest keeps `review.status: pending` for all models until then.
- **Consent statements.** Only the BlazeFace card states consented training images. The Face Mesh v2, Blendshape and multiclass cards are silent. This is recorded in each manifest entry's notes.
- **Provenance evidence (2026-10-01):**
  - AuraFace's `glintr100.onnx` is **not** byte-identical to antelopev2's.
  - Its `scrfd_10g_bnkps.onnx` **is** identical to antelopev2's, which confirms we must never ship it.
  - The lab build fails if any InsightFace file name appears.
