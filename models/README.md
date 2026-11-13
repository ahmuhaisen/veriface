# models/

Model binaries are **not** committed. `manifest.json` is committed, and the lab generates it (`python -m facelab models build`).

Each entry records:
- the source URL
- the licence and training-data statement
- the legal review status
- the SHA-256 hash
- the tensor I/O spec

The service verifies every hash at startup and refuses to start on a mismatch.

Never place InsightFace / antelopev2 / buffalo files here. That includes the non-recognition ONNX files bundled in the AuraFace repository. See `docs/decisions/0002-models.md`.
