# lab/ (facelab)

Internal Python tooling for Veriface. **Never shipped.** It produces the model pack, the golden outputs and the calibration reports that the Rust service consumes (`docs/architecture.md` §6).

## Setup (Windows, Python 3.12): two environments

The two tool chains need incompatible protobuf and numpy versions, so the lab uses two venvs:

| venv | Install | Lock file | Used for |
|---|---|---|---|
| `lab/.venv` | `-e "lab[convert,dev]"` | `requirements.lock.txt` | `models fetch/build/bom`, fidelity, `experiment auraface-int8`. Contains TensorFlow, tf2onnx and onnx. **No MediaPipe.** |
| `lab/.venv-mp` | `-e "lab[mp,dev]"` | `requirements-mp.lock.txt` | `testdata documents`, `golden`, `eval`, `parity`. Contains **mediapipe 0.10.21** (pinned, telemetry-free). |

```bash
py -3.12 -m venv lab/.venv
lab/.venv/Scripts/python.exe -m pip install -r lab/requirements.lock.txt
lab/.venv/Scripts/python.exe -m pip install -e "lab[convert,dev]" --no-deps

py -3.12 -m venv lab/.venv-mp
lab/.venv-mp/Scripts/python.exe -m pip install -r lab/requirements-mp.lock.txt
lab/.venv-mp/Scripts/python.exe -m pip install -e "lab[mp,dev]" --no-deps
```

Both venvs use **onnxruntime 1.28.0**, the closest PyPI wheel to the service's ONNX Runtime 1.28.2.

## Privacy: MediaPipe telemetry

**Only the telemetry-free MediaPipe may run in the lab.** `mediapipe` 0.10.35 and 1.0.x for Python embed Google's Clearcut logger (`https://play.googleapis.com/log`) in their native library. During local-only inference they opened connections to Google about 60 s in.

`mediapipe` **0.10.21** (and 0.10.14) contain no Clearcut code. Each contacted no remote endpoint in a 180 s probe. Probe date: 2026-10-01; probe script: `src/facelab/experiments/telemetry_probe.py`, which runs standalone in any venv.

`facelab/mptasks.py` refuses to import with any MediaPipe version not on `TELEMETRY_FREE_MEDIAPIPE`. Add a version there only after re-running the probe on it.

Consequence: 0.10.21's Tasks FaceDetector cannot run the full-range model. Full-range detection in the goldens therefore uses the lab reference detector (`src/facelab/ref/`), which matched MediaPipe Tasks 1.0.1 on every fixture.

## Commands

| Command | What it does |
|---|---|
| `python -m facelab models fetch` | Downloads the pinned upstream artifacts into `models/upstream/` (git-ignored) and verifies each SHA-256. A mismatch is a hard failure. |
| `python -m facelab models build` | Fetches (cached), converts the TFLite models to ONNX, runs the fidelity check (L3), and writes `models/*.onnx`, `models/mp-canonical-face.json` and `models/manifest.json`. Any fidelity failure means no manifest is written. |
| `python -m facelab models pack --version X [--out DIR]` | Writes the versioned model pack `veriface-models-X.zip` (+ `.zip.sha256`), default `service/dist/` (git-ignored). Contents: models + manifest (byte-identical), `licenses/models/<id>/` full upstream licence texts, `NOTICE` (from manifest `licence.attribution`), `THIRD-PARTY-LICENSES.txt`, `third-party-models.json`, `PACK.json`, `SHA256SUMS`. Byte-reproducible. Consumed by `service/scripts/package.ps1 -ModelPack` and the Docker `full` image (`.venv`). |
| `python -m facelab models bom` | Prints the model bill of materials (licence, training-data statement, review) as markdown |
| `python -m facelab testdata portraits` | Fetches the pinned PD NASA portraits from Wikimedia Commons, checks the `{{PD-USGov-NASA}}` tag and the original SHA-1, downscales them, writes the pose fixtures, and updates `testdata/provenance.json` and `testdata/LICENSES.md` |
| `python -m facelab testdata documents` | Synthesises the SPECIMEN ID-1 documents and their ground truth |
| `python -m facelab eval run --manifest M --out DIR [--match-on sex,ageBand,skinTone] [--scores CSV] [--predictions JSONL]` | L7 evaluation and calibration. The manifest format is in `src/facelab/eval/dataset.py`. Data root and output must be **outside the repo** (or the output under `lab/work/`); a consent reference is required. Writes `report.md`, `results.json`, DET CSV/PNG and proposed profile values. |
| `python -m facelab eval fixtures` | **Illustrative** end-to-end run on the PD fixtures, written to `lab/work/eval-fixtures/`. Not a calibration. |
| `python -m facelab experiment auraface-int8` | L6: INT8 AuraFace vs FP32 (size, latency, drift), written to `lab/work/` |
| `python -m facelab parity --service URL [--api-key-env VAR] [--profile P]` | L8: runs the fixtures through the service API and compares the results with the goldens (`.venv-mp`) |
| `python -m facelab golden [ids...]` | Writes `testdata/golden/` (L5). Accepts only provenance-listed images. Format: `testdata/golden/README.md` |

## Release check

`lab/scripts/release-parity.ps1 -Service URL [-ApiKeyEnv VAR] [-Profile fixtures] [-Out DIR]` runs `facelab parity --require-complete` for the release-check script. Exit codes: 0 gate passed, 1 gate failed or the run crashed, 3 NOT RUN because a prerequisite is missing (`.venv-mp`, models, goldens, key, a ready service, or the profile; the reason goes to stderr). Exit 3 never counts as passed. The `fixtures` profile is dev-only (`tools/console/scripts/dev-setup.mjs`); without it score parity is not exercised.

## Reproducibility

`models build` is byte-reproducible: two builds give identical model hashes, and only `generatedAt` changes. tf2onnx's graph optimizer is not deterministic, so it is disabled, and internal tensor names are canonicalised. Weights are never modified. See `src/facelab/convert.py`.

## Licence guard

From the AuraFace repository, only `glintr100.onnx` (the recognition model) is ever fetched. `upstream.FORBIDDEN_NAMES` makes fetch and build fail if any InsightFace detector or landmark file shows up in the cache.

## Checks

```bash
lab/.venv/Scripts/python.exe -m ruff check lab/src lab/tests
lab/.venv/Scripts/python.exe -m ruff format --check lab/src lab/tests
lab/.venv/Scripts/python.exe -m pytest lab       # conversion-side tests (MediaPipe tests skip)
lab/.venv-mp/Scripts/python.exe -m pytest lab    # MediaPipe-side tests (conversion tests skip)
```

`python -m facelab.experiments.golden_drift <old golden dir>` compares a golden set with `testdata/golden/` field by field. Use it after any runtime pin change.
