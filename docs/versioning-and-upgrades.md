# Versioning and upgrades

Veriface has five independently versioned parts. A result can only be reproduced or explained
if all five are known, so every response reports them (`engine.version`,
`engine.catalogueVersion`, `profile.version` and `profile.sha256`, the model `sha256` values in
`engine.models`) and `/v1/info` lists them.

| Part | Version | Where it is visible | Owner |
|---|---|---|---|
| Binary (`veriface`) | SemVer, e.g. `0.1.0` | `engine.version`, `veriface --version`, workspace `Cargo.toml` | Lead |
| HTTP API | Path prefix `/v1`; `info.version` in `contracts/openapi.yaml` (`1.0.0`) | URL | Lead |
| Check catalogue | Integer `catalogueVersion` (`1`) | `engine.catalogueVersion`, `contracts/checks.yaml`, every profile | Lead |
| Profiles | SemVer per profile, e.g. `standard` `0.1.0`, plus the file's SHA-256 | `profile.version`, `profile.sha256` | Lead (thresholds from lab calibration) |
| Model pack | `models/manifest.json`: `manifestVersion`, `generatedAt`, `generator`, and a SHA-256 per file | `engine.models[].sha256`, `reviewStatus` | Lab (face-dev-1) |

The SDK (`@veriface/capture`) has its own package version and follows the same SemVer rules
for its JavaScript API. Its default thresholds track `profiles/standard.yaml`; a unit test fails
when they drift.

## 1. The binary: SemVer

`MAJOR.MINOR.PATCH`, from `[workspace.package] version` in `service/Cargo.toml`.

| Bump | When |
|---|---|
| MAJOR | A breaking change for operators or callers: a new API major version (`/v2`) becomes the only one, a configuration key is removed or changes meaning, a platform is dropped, or the catalogue version changes in a way that old profiles stop loading |
| MINOR | New features that keep existing configurations and calls working: a new optional config key with a safe default, a new endpoint, a new optional response field, support for a new catalogue version alongside the old one |
| PATCH | Fixes with no change to configuration or contracts. Results for the same inputs may change only where they were wrong (the release notes say so) |

While the version is `0.x`, the project is pre-release: MINOR releases may still break things, and
the release notes call that out explicitly.

The binary embeds the check catalogue (`contracts/checks.yaml`) at build time, so a binary
supports exactly the catalogue versions it was built with.

## 2. The API: `/v1` and breaking-change rules

The URL prefix is the API's major version. Inside `/v1`, changes must be **backward compatible**.

Allowed within `/v1`:

- New endpoints.
- New **optional** query parameters with a default that keeps today's behaviour.
- New response fields. Clients must ignore unknown fields (the examples in `examples/` do).
- New problem `code` values for new error conditions. Clients must treat an unknown code by its
  HTTP status.
- New check ids, as part of a catalogue change (section 3). Clients must handle check ids they
  do not know (show a generic message, see the integration guide section 7).
- New values in `engine.models`.

Breaking, so only in a new major path (`/v2`):

- Removing or renaming an endpoint, a parameter, a response field or a multipart part name.
- Changing a field's type or meaning, or making a nullable field non-nullable or the reverse.
- Changing the meaning of `verdict`, `decision` or `outcome`, or adding a new **enum value** to
  them (`outcome` in particular: projects branch on it).
- Changing which HTTP status a condition returns.
- Turning an optional parameter into a required one.

When `/v2` arrives, `/v1` stays available for a deprecation period announced in the release notes
(at least one MINOR release of overlap), so projects can migrate.

`contracts/openapi.yaml` `info.version` tracks the contract document: MINOR for compatible
additions, PATCH for clarifications, MAJOR together with a new path prefix.

## 3. The check catalogue: `catalogueVersion`

`contracts/checks.yaml` declares `catalogueVersion: 1`. Every profile declares the catalogue
version it was written for, and the service refuses a profile whose `catalogueVersion` does not
match its own (`catalogueVersion N does not match the service catalogue version M`).

Bump `catalogueVersion` (and tell the lead) when:

- a check id is renamed or removed;
- a check's `value` semantics or `unit` change (for example a score that used to mean "higher is
  better" now means the opposite, or degrees become radians);
- prerequisites, subjects, the verdict rule, the reason priority or the verification
  `decision`/`outcome` rules change.

Adding a new check id is also a catalogue change: profiles must be able to list it, and clients
see new ids. It needs a catalogue bump and new profile versions.

A catalogue bump forces new profile versions and usually a new binary MINOR or MAJOR version
(section 1). It may be breaking for clients that map check ids to messages; it is announced in the
release notes, with the new ids and their meaning.

Not a catalogue change: rewording descriptions without changing meaning, or changing a
`defaultSeverity` that every shipped profile overrides anyway (still noted in the release notes).

## 4. Profiles: their own SemVer

Each profile file carries `name` and `version` (`^\d+\.\d+\.\d+$`). Responses report
`profile.version` and `profile.sha256` (the SHA-256 of the file bytes), so any edit, even a
comment, is visible.

| Bump | When |
|---|---|
| MAJOR | Behaviour changes in a way that alters outcomes broadly: a new catalogue version, a different `verification.model`, checks enabled or disabled, a check's severity changed, required probe/reference checks changed, operating points removed or renamed, or `defaultOperatingPoint` changed |
| MINOR | Thresholds moved (including the calibration that turns `provisional` into `calibrated`), a new operating point added, output settings changed |
| PATCH | No effect on results: comments, `description`, calibration notes |

Rules:

- **Never change a shipped profile without bumping its version.** Projects record
  `profile.version` and `profile.sha256` with their decisions.
- **A profile is tied to a model pack.** Thresholds are only meaningful for the embedder they were
  calibrated on. `verification.model` names it, and the service refuses a profile whose model is
  not the loaded embedder.
- **Calibration is recorded in the profile** (`verification.calibration`: `id`, `status`,
  `dataset`, `date`, `notes`). Responses carry `calibration.id` and `status`.
- A project that needs different thresholds gets a **new profile name**, not an edited `standard`.

## 5. The model pack: manifest and hashes

`models/manifest.json` lists every model and asset with its file name, byte size, SHA-256, upstream
source and hash, licence, training-data statement, review status and I/O specification
(`contracts/model-manifest.schema.json`, `manifestVersion: 1`). The lab's `facelab models build` is
byte-reproducible: two builds give identical model hashes, only `generatedAt` changes.

| Rule | Detail |
|---|---|
| The hashes are the version | A model pack is identified by its file hashes. Two packs with the same hashes are the same pack |
| Verified at startup | Every file's size and SHA-256 must match the manifest, or the service does not start. A `rejected` review status is refused |
| No silent swaps | Never replace a model file or edit the manifest by hand on a server |
| **Model changes require recalibration** | Any change to the face embedder (new model, retrained or fine-tuned weights, different precision such as INT8, or a different conversion) changes the score distribution. Every operating point threshold must be recalibrated, the profiles get a new MAJOR version, and their calibration status restarts at `provisional` unless the lab has measured the new model. A change to a detector, landmarker or segmenter changes quality measurements; quality thresholds need re-checking and the golden tests must pass |
| Pack and profiles ship together | A release names the model pack its profiles were calibrated for |
| Manifest format | `manifestVersion` changes only when the manifest's own structure changes. Version 1 is the only one defined today (`contracts/model-manifest.schema.json`) |

The ONNX Runtime library is part of the runtime too: the service requires 1.28.x and can pin the
exact file (`engine.onnxruntime_sha256`). Changing the ONNX Runtime minor version is a binary
release with golden-test and parity checks (`facelab parity`, `golden_drift`).

## 6. Compatibility matrix

Each release states, in `CHANGELOG.md`:

| Field | Example |
|---|---|
| Binary version | `0.1.0` |
| API | `/v1` (`openapi.yaml` `1.0.0`) |
| Catalogue version | `1` |
| Profiles shipped | `standard 0.1.0`, `icao-strict 0.1.0` |
| Model pack | manifest `generatedAt` and the SHA-256 of each model (or of `manifest.json`) |
| ONNX Runtime | `1.28.2` |
| SDK | `@veriface/capture 0.1.0` |

**`[PLANNED: CI produces the matrix and the release checksums automatically. Placeholder for the
CI pipeline and release artifact names.]`**

## 7. Upgrade principles

1. **Read the release notes first.** Know which of the five parts changed.
2. **Upgrade as a set.** Binary, profiles and model pack from the same release. Do not mix parts
   across releases unless the release notes say they are compatible.
3. **Verify before installing.** Check `SHA256SUMS` on the copied package. The service then checks
   the models itself.
4. **Install side by side.** Keep the previous release's directory untouched, with its own
   configuration, profiles and model pack.
5. **Let the service validate.** Run the new release in the foreground or as a test service on
   another port; it lists every configuration, profile and model problem at startup.
6. **Tell projects about outcome-affecting changes in advance.** A new profile version or model
   pack can change outcomes for the same images. Projects must record `profile.version`,
   `profile.sha256` and `calibration.id` with each decision.
7. **Watch after the switch.** `/health/ready`, error rate, latency, and the outcome and check
   mixes (`face_verification_outcome_total`, `face_check_status_total`). A shift after a
   threshold change is expected; a shift after a binary-only patch is a bug.
8. **Configuration keys.** New keys come with defaults that keep today's behaviour. Unknown keys
   are errors, so a configuration written for a newer release fails clearly on an older binary.

## 8. Rollback principles

1. **Rollback = start the previous release as it was**: its binary, configuration, profiles and
   model pack, from its untouched directory.
2. **No data migration is needed.** The service is stateless and stores nothing, so there is
   nothing to convert or restore.
3. **Decisions made during the bad window** are identifiable: every result carries the engine
   version, profile version and hash, and calibration id. Projects can re-run or review those
   cases if needed.
4. **Keep at least the previous release** (package plus checksums) on hand until the new one has
   run in production long enough to trust it.

**`[PLANNED: installer upgrade and rollback commands. Placeholder for the installer's in-place
upgrade, side-by-side install and rollback procedure.]`**
