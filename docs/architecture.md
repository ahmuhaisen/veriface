# Architecture

Contracts in `contracts/` are authoritative. This document explains how the system meets them.

## 1. Components

| Component | Role | Ships? |
|---|---|---|
| `service/` (Rust) | HTTP API plus the inference pipeline. One binary: `veriface` | Yes |
| `models/` | ONNX models plus `manifest.json` (SHA-256, licence, I/O spec) | Yes (model pack) |
| `profiles/` | Threshold profiles (YAML, validated against `contracts/profile.schema.json`) | Yes |
| `sdk-web/` (TypeScript) | In-browser capture guidance and auto-capture. Never calls the service | Yes (npm package) |
| `lab/` (Python) | Model conversion, golden outputs, evaluation, calibration, parity tests | No |

**Runtime:**
- ONNX Runtime, through the `ort` crate with `load-dynamic`.
- It loads Microsoft's official `onnxruntime.dll` (Windows) or `libonnxruntime.so` (Linux), with the version pinned in the release.
- CPU execution provider only.

## 2. Models (v1)

| Id (manifest) | Upstream | Role | Licence |
|---|---|---|---|
| `mp-face-detector-short` | MediaPipe BlazeFace short-range | Face detection in selfies (128x128) | Apache-2.0 |
| `mp-face-detector-full` | MediaPipe BlazeFace full-range | Small faces: counting background faces, document portraits | Apache-2.0 |
| `mp-face-landmarker` | MediaPipe Face Landmarker (face_landmarks_detector) | 478 landmarks including iris | Apache-2.0 |
| `mp-face-blendshapes` | MediaPipe face_blendshapes | 52 expression coefficients (eye blink, jaw open, smile, eye look) | Apache-2.0 |
| `mp-selfie-multiclass` | MediaPipe selfie_multiclass_256x256 | Segmentation into background, hair, body-skin, face-skin, clothes and accessories | Apache-2.0 |
| `auraface-v1` | fal/AuraFace-v1 (recognition model only) | 512-d face embedding (ResNet-100) | Apache-2.0. Training data undisclosed, so legal review is **pending** |
| asset `mp-canonical-face` | MediaPipe canonical face model | 3-D reference geometry for the pose fit | Apache-2.0 |

The lab converts the MediaPipe TFLite models to ONNX. It checks each converted model against the original runtime (fidelity) and records the result in the manifest.

## 3. Pipelines

### 3.1 Selfie assessment (`/v1/quality`, verification probe)

1. **Decode.**
   - Check `maxBytes`, then read the image header and check `maxPixels` *before* full decode.
   - Decode with memory-safe decoders and apply EXIF orientation. Convert to RGB8, then drop all metadata.
   - Record JPEG quantisation tables for `COMPRESSION_QUALITY`.
2. **Detect.**
   - Run BlazeFace full-range on a letterboxed copy, for counting and for the primary-face candidate.
   - Run short-range on the primary region when the face is close. The vision owner tunes this.
   - The primary face is the one with the largest inter-eye distance.
3. **Landmarks.** Build a rotated ROI from the detection keypoints and warp it to the landmark model's input size. The model returns 478 points (x, y, z) and a presence score.
4. **Blendshapes.** Input is the model's subset of 146 landmarks; output is 52 coefficients.
5. **Pose.** Use a weighted, **scaled-orthographic** (weak-perspective) rigid Procrustes fit of the 3-D landmarks to the canonical face, exactly as `definitions.pose` in `checks.yaml` specifies. Then convert to yaw / pitch / roll with the sign conventions there.
   - Pose is relative to the camera-to-face line of sight. It must not depend on where the face sits in the frame.
   - MediaPipe's perspective `facial_transformation_matrix` is **not** used. It assumes a camera field of view, and that adds a position-dependent bias of roughly a third of the face's line-of-sight angle: about −6° pitch for faces in the upper third of a portrait.
   - **Pinned decomposition:** R = Rz(c) · Ry(b) · Rx(a), intrinsic, with yaw = +b, roll = −c, pitch = −a.
   - The lab's Python **weak-perspective fit** is the reference. The goldens record its angles as `pose`; MediaPipe's matrix-derived angles are kept for information only. The Rust service and the web SDK must reproduce it within the golden tolerances.
6. **Segmentation.** Run the multiclass segmenter on a head-and-shoulders crop. Classes feed `EYES_VISIBLE`, `FACE_OCCLUSION`, `HEAD_COVERING`, `BACKGROUND_UNIFORMITY` and the crown estimate for the head box.
7. **Measurements.** Each check produces one number, or `None` with a cause. Image measures use the face-skin region; sharpness is measured after normalising the face to a fixed inter-eye distance.
8. **Evaluation (`face-core`).** Apply profile ranges, prerequisites and severities. This produces statuses, the verdict, and `reasons` / `warnings` in catalogue priority order.
9. **Output.**
   - Applies when both `FACE_PRESENT` and `SINGLE_FACE` pass.
   - `portrait-3x4` places the eye midpoint at (0.50 W, 0.42 H), with head height (chin to crown) about 0.72 H.
   - If that crop leaves the image, shift and shrink it to fit. Never pad with invented pixels.
   - Encode JPEG starting at quality 92 and step down by 5 until it fits `maxBytes` (floor `minJpegQuality`). No metadata is written.

### 3.2 Document portrait (`/v1/document-portraits`, verification reference)

1. **Decode.** Same as 3.1.
2. **Rectify.**
   - Downscale to about 1024 px on the long side. Find edges and fit the largest convex quadrilateral whose aspect ratio is close to ID-1 (1.586) or TD3 (about 1.42).
   - Perspective-warp it from the *full-resolution* source. Never upscale. The rectified ID-1 frame is 1011×638 scaled by k = min(1, shorter horizontal edge / 1011, shorter vertical edge / 638), measured on the source quad. The goldens' `iodDocumentFrame` depends on this rule (`golden.document_frame_scale`).
   - Report the confidence as `DOCUMENT_DETECTED`.
   - When `referenceKind=portrait`, skip this step.
3. **Orientation.** Run the face detector on the four 90-degree rotations and keep the rotation with the strongest large face.
4. **Portrait selection.** The largest face by inter-eye distance; smaller "ghost" portraits are ignored. `PORTRAIT_FOUND` = number of candidates.
5. **Portrait analysis.** Landmarks and pose (plus segmentation where the profile needs it) on a crop around the portrait, then the `portrait`-subject measurements.
6. **Evaluation and output.** As in 3.1. The `face-crop` style never upscales beyond the source.

### 3.3 Verification (`/v1/verifications`)

1. Assess the probe with 3.1 and the reference with 3.2.
2. **Alignment.** A five-point similarity transform to the standard 112x112 ArcFace template. The five points are pinned below; the lab, the service and `landmarks5` in the API all use them.

   | Order (ArcFace template) | Point | MediaPipe 478-landmark index |
   |---|---|---|
   | 1 | Eye centre, image-left (subject's right eye) | 468 (iris centre) |
   | 2 | Eye centre, image-right (subject's left eye) | 473 (iris centre) |
   | 3 | Nose tip | 1 |
   | 4 | Mouth corner, image-left (subject's right) | 61 |
   | 5 | Mouth corner, image-right (subject's left) | 291 |

   Template (112x112): (38.2946, 51.6963), (73.5318, 51.5014), (56.0252, 71.7366), (41.5493, 92.3655), (70.7299, 92.2041).
3. **Embedding.** AuraFace, then L2-normalise. The input colour order and normalisation come from `manifest.json` `io`.
4. **Score.** Cosine similarity. Compare it with the selected operating point's threshold to get `decision`, then apply the `outcome` rules in `checks.yaml`.
5. **Risk checks.**

   | Check | Signals |
   |---|---|
   | `PROBE_REFERENCE_DUPLICATE` | Similarity above the genuine selfie-vs-document range; near-identical aligned crops (SSIM); near-identical pose |
   | `PROBE_DOCUMENT_PRESENT` | The rectification detector run on the selfie: a card-shaped quadrilateral enclosing the face |
   | `PROBE_SCREEN_ARTIFACTS` | Frequency-domain peaks (moire or pixel grid) in the face region. Experimental |

## 4. Rust crate layout (`service/crates/`)

Dependencies point **downward only**.

| Crate | Owns | Depends on |
|---|---|---|
| `face-core` | Catalogue (embedded from `contracts/checks.yaml`), profiles and their validation, measurements to statuses to verdict / reasons, verification decision and outcome. **No ML, no I/O.** | — |
| `face-imaging` | Safe decode with limits and EXIF orientation, JPEG quality estimate, colour conversion, resize, affine and projective warp, crop, JPEG encode to a byte budget, image-statistics helpers | — |
| `face-models` | ORT runtime and session pool, manifest loading and SHA-256 verification, wrappers for detector, landmarker, blendshapes and segmenter (pre- and post-processing) | `face-imaging` |
| `face-quality` | Pose fit, geometry, illumination / sharpness / background measures, selfie and portrait measurements, output crop | `face-core`, `face-imaging`, `face-models` |
| `face-document` | Rectification, orientation search, portrait selection, document measurements | `face-core`, `face-imaging`, `face-models`, `face-quality` |
| `face-verify` | AuraFace embedder and alignment, similarity, risk checks, verification assembly | `face-core`, `face-imaging`, `face-models`, `face-quality`, `face-document` |
| `face-server` (bin `veriface`) | HTTP (axum), multipart, API keys, limits, RFC 9457, health / metrics, config, Windows service, wiring | all |

The contract between the vision crates and `face-core` is a **`Measurement` per check id**: a value, or `None` with a cause. `face-core` alone decides statuses, verdicts and ordering, so the contract logic lives in one tested place.

## 5. Runtime behaviour

- **Concurrency.**
  - CPU work runs on a blocking pool behind a semaphore (`max_concurrency`; 0 = available parallelism, i.e. logical CPUs).
  - Requests that would exceed `queue_limit` get `503 overloaded` with `Retry-After`.
  - ORT sessions are created once and shared, with `intra_op_num_threads` defaulting to 1 so parallelism happens at request level.
- **Startup.**
  1. Validate the config.
  2. Load the manifest and verify every model's SHA-256. On a mismatch, refuse to start.
  3. Validate profiles against the schema and the catalogue. Unknown check ids are a startup error.
  4. Create the sessions and run a warm-up inference per model.
  5. Only then does `/health/ready` return 200.
- **Config.** A TOML file holding the bind address, paths, concurrency, limits, and API keys (stored as SHA-256 hashes, each with allowed profiles and a default profile), plus metrics and logging settings.
- **Windows service.**
  - `veriface service install --config <path>` registers it with the Service Control Manager; `service uninstall` removes it; `run` runs in the foreground.
  - Linux uses the same binary in a container.
- **Observability.**
  - JSON logs: request id, endpoint, verdict, failed check ids, timings. Never images or embeddings.
  - Prometheus metrics: `face_requests_total`, `face_request_duration_seconds`, `face_stage_duration_seconds`, `face_check_status_total{check,status}`, `face_verification_outcome_total`, `face_inflight`, `face_queue_depth`.

## 6. Lab ↔ service hand-offs

| Artifact | Producer | Consumer |
|---|---|---|
| `models/*.onnx` + `models/manifest.json` | lab (`facelab models build`) | service (startup) |
| `testdata/images/*` + `testdata/LICENSES.md` | lab | Rust tests, SDK demo |
| `testdata/golden/*.json`: official MediaPipe / ONNX reference outputs, with tolerances | lab | Rust golden tests |
| Calibrated thresholds plus a calibration report | lab | lead, who applies them to `profiles/` |
| Parity report: service output vs lab expectations on the test set | lab (`facelab parity`) | release gate |
