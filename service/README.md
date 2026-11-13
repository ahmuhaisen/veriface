# Veriface service (`veriface`)

The Rust HTTP service: quality checks, document portraits and 1:1 verification, exactly as
specified in [`contracts/openapi.yaml`](../contracts/openapi.yaml). Design:
[`docs/architecture.md`](../docs/architecture.md). Team rules: [`AGENTS.md`](../AGENTS.md).

The engine (`crates/face-server/src/pipeline.rs`) runs the vision crates: MediaPipe detection,
landmarks, blendshapes and segmentation (`face-models`, `face-quality`), document rectification
and portrait selection (`face-document`), and AuraFace embedding with the risk checks
(`face-verify`). `[engine] kind = "stub"` swaps in a model-free stub (every check
`undetermined`, `engine.version` ending in `+stub`) for tests and smoke runs only.

## Build and test

Prerequisites: the Rust toolchain pinned in `rust-toolchain.toml` (installed automatically by
`rustup`) and, on Windows, the Visual Studio C++ Build Tools (MSVC linker and Windows SDK).

```powershell
cd service
cargo build --release -p face-server          # target\release\veriface.exe
cargo test -p face-server -p face-imaging   # real-engine tests skip without ORT and models
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

When several people or agents build at once, give each its own target directory
(`$env:CARGO_TARGET_DIR = "target-<name>"`) to avoid waiting on cargo's lock.

## Files the service needs

| What | Where (default, relative to the config file) | Notes |
|---|---|---|
| `veriface.exe` / `veriface` | anywhere | One binary; no installer |
| Configuration | e.g. `config.toml` | Copy [`config.example.toml`](config.example.toml) |
| Profiles | `profiles/*.yaml` | Validated at startup; their SHA-256 is reported as `profile.sha256` |
| Models | `models/manifest.json` + the files it lists | Produced by the lab; checked by SHA-256 at startup |
| ONNX Runtime | `onnxruntime.dll` (Windows), `libonnxruntime.so.1.28.2` (Linux) | Microsoft's official build, see below |

### ONNX Runtime library

The service loads **Microsoft's official ONNX Runtime 1.28** at runtime (`ort` crate with
`load-dynamic`). Fetch and verify it with the script maintained by the vision team:

```powershell
powershell -ExecutionPolicy Bypass -File service/scripts/fetch-onnxruntime.ps1      # win-x64
powershell -ExecutionPolicy Bypass -File service/scripts/fetch-onnxruntime.ps1 -Platform linux-x64
```

```bash
service/scripts/fetch-onnxruntime.sh
```

It lands in `service/third_party/onnxruntime/1.28.2/<platform>/` (git-ignored). Copy
`onnxruntime.dll` next to `veriface.exe` (the packaging script does this) or point
`paths.ort_library` at it. Do **not** use the older `onnxruntime.dll` that Windows ships in
`System32`: the API version check refuses it. Ship ONNX Runtime's `LICENSE` and
`ThirdPartyNotices.txt` with it (the packaging script copies them to `licenses/onnxruntime/`).

## Configuration

A TOML file; [`config.example.toml`](config.example.toml) documents every key. Relative paths are
resolved against the directory of the configuration file, so the service behaves the same from
any working directory (a Windows service starts in `System32`). Unknown keys are errors.

| Key | Default | Meaning |
|---|---|---|
| `health_requires_key` | `false` | Also require `X-API-Key` on `/health/live` and `/health/ready`. Must appear before the first `[table]` |
| `server.bind` | `127.0.0.1:8080` | `ip:port` to listen on (plain HTTP; terminate TLS in front) |
| `server.max_concurrency` | `0` | Requests processed at once; `0` = logical CPUs |
| `server.queue_limit` | `32` | Requests that may wait for a slot; beyond that `503 overloaded` |
| `server.request_timeout_ms` | `30000` | Deadline for a whole request; `503 overloaded` when exceeded |
| `paths.models` | required | Directory with `manifest.json` and the models |
| `paths.profiles` | required | Directory of `*.yaml` profiles |
| `paths.ort_library` | see below | Path to the ONNX Runtime library; when unset, next to the executable, then `third_party/onnxruntime/1.28.2/<platform>/` next to the executable or the config |
| `[[api_keys]]` | at least one (here or in `api_keys_file`) | `project`, `sha256`, `profiles` (`["*"]` = all), optional `default_profile` |
| `api_keys_file` | none | A TOML file holding more `[[api_keys]]` entries and nothing else (e.g. a mounted secret); added to those of the main file. Top-level key |
| `engine.kind` | `pipeline` | `pipeline` (the models) or `stub` (tests and smoke runs only) |
| `engine.intra_op_threads` | `1` | ONNX Runtime threads per inference |
| `engine.intra_op_threads_per_model` | `auraface-v1 = 2`, `mp-selfie-multiclass = 2` | Per-model thread overrides; a table in the file replaces the default, an empty one turns it off |
| `engine.sessions_per_model` | `0` | Sessions per model; `0` = `max_concurrency`, capped at 4 (about 290 MB each) |
| `engine.parallel_stages` | `true` | Run a verification's independent stages concurrently (same results, lower latency) |
| `engine.onnxruntime_sha256` | none | Optional pin of the ONNX Runtime library file |
| `metrics.enabled` | `true` | Serve `GET /metrics` (no authentication) |
| `logging.format` | `json` | `json` or `pretty` |
| `logging.level` | `info` | `tracing` filter directives, e.g. `info,face_server=debug` |
| `logging.file` | none | Append logs to this file instead of stdout (needed for the Windows service) |

At startup the service validates the configuration, loads ONNX Runtime, verifies every model
file against `models/manifest.json` (SHA-256; `rejected` models are refused), creates the
sessions, loads and validates every profile (each profile's `verification.model` must be the
loaded embedder), checks every API key's profiles, binds, runs one warm-up inference per model
and only then reports ready. **Any problem stops it, and it lists every problem it found**, e.g.

```text
veriface: invalid configuration: 2 problems:
  - config.toml: server.bind `localhost` is not an `ip:port` address (e.g. 0.0.0.0:8080)
  - config.toml: api_keys[0] (crm): sha256 must be 64 hex characters (the SHA-256 of the key, not the key)
```

`veriface check --config config.toml` runs all of these checks (configuration, ONNX Runtime,
models, profiles, keys) and exits: 0 when the service would start, 2 with the list of problems
otherwise. The installers run it before every install and upgrade.

### Environment overrides

Any key of the file can be overridden with `VERIFACE__<TABLE>__<KEY>=value` (`__` between
levels, case-insensitive), which suits containers:

```bash
VERIFACE__SERVER__BIND=0.0.0.0:8080
VERIFACE__SERVER__MAX_CONCURRENCY=8
VERIFACE__PATHS__MODELS=/models
VERIFACE__API_KEYS_FILE=/run/secrets/veriface-keys
VERIFACE__LOGGING__FILE=          # empty: remove the key (log to stdout)
```

Values are read as TOML booleans, numbers or arrays when they are one (`true`, `8`,
`["a","b"]`), otherwise as strings. `[[api_keys]]` cannot be set this way (use
`api_keys_file`). Unknown keys are errors, as in the file, and the message names the variables.

### API keys

The configuration stores only the SHA-256 (hex) of each key. Generate a random key and its hash,
give the key to the integrating project, and put the hash in `[[api_keys]]`:

```powershell
$bytes = New-Object byte[] 32
[Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
$key = [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
$sha = [Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($key))
"key:    $key"
"sha256: " + (($sha | ForEach-Object { $_.ToString('x2') }) -join '')
```

```bash
key=$(openssl rand -base64 32 | tr '+/' '-_' | tr -d '=')
echo "key:    $key"
echo "sha256: $(printf '%s' "$key" | sha256sum | cut -d' ' -f1)"
```

Each key lists the profiles it may use. A request without `?profile=` uses the key's
`default_profile`, else `standard`. A profile the key may not use is `403 profile-forbidden`; an
unknown one is `404 profile-not-found`.

## Running

```powershell
veriface run --config C:\veriface\config.toml
```

Ctrl-C (and SIGTERM on Linux) stops it gracefully: it stops accepting connections, reports not
ready, and lets in-flight requests finish (at most `request_timeout_ms` plus 5 seconds).

```bash
curl -s localhost:8080/health/ready
curl -s -H "X-API-Key: $KEY" -F image=@selfie.jpg "localhost:8080/v1/quality?profile=standard"
curl -s -H "X-API-Key: $KEY" -F probe=@selfie.jpg -F reference=@id-card.jpg localhost:8080/v1/verifications
```

`veriface healthcheck --config config.toml` (or `--url http://127.0.0.1:8080/health/ready`)
probes a running service and exits 0 on 200, 1 otherwise; it is the container `HEALTHCHECK`
(the runtime image has no shell or curl). With `health_requires_key = true`, set
`VERIFACE_HEALTHCHECK_KEY` to a key.

## Deployment

| Target | How | Guide |
|---|---|---|
| Windows Server 2019/2022/2025 | Versioned zip + `Install-Veriface.ps1` (install, side-by-side upgrade with automatic rollback, rollback, uninstall; virtual service account, ACLs, firewall, recovery) | [`deploy/windows/README.md`](../deploy/windows/README.md) |
| Docker | `slim` (models mounted) and `full` (models baked in) images, distroless, non-root, read-only; compose file; offline `docker save` tarballs | [`deploy/docker/README.md`](../deploy/docker/README.md) |
| Linux (bare metal) | Tarball + hardened systemd unit | [`deploy/linux/README.md`](../deploy/linux/README.md) |

## Windows service (low level)

`deploy/windows/Install-Veriface.ps1` is the supported way to install; it calls the commands
below and adds the account, ACLs, firewall rule and side-by-side upgrades. From an **elevated**
prompt:

```powershell
veriface.exe service install --config C:\veriface\config.toml        # [--name veriface] [--account ...]
Start-Service veriface        # or: sc.exe start veriface
Stop-Service veriface         # graceful: in-flight requests finish first
veriface.exe service uninstall                                           # [--name veriface]
```

- `install` validates the configuration and profiles first, so a broken configuration is caught
  before the service exists. It stores the configuration path as an absolute path.
- The service starts automatically at boot, and the Service Control Manager restarts it after a
  failure (after 5 s, 30 s, then 120 s; the counter resets after a day).
- It runs as its own virtual account `NT SERVICE\<name>` by default (least privilege, no
  password). That account must be able to read the install and configuration directories and
  write the log directory (the installer grants exactly that):

  ```powershell
  icacls "C:\ProgramData\Veriface\logs" /grant "NT SERVICE\veriface:(OI)(CI)M"
  ```

  `--account "NT AUTHORITY\LocalService"`, a gMSA (`--account "DOMAIN\name$"`) or
  `--account LocalSystem` (not recommended) also work.
- A Windows service has no console: set `logging.file` (e.g. `logs/veriface.log`) or there are
  no logs. Startup failures also appear in the Windows event log (Service Control Manager).

## Linux

The same binary, without the `service` commands. It stops gracefully on SIGTERM. See
[`deploy/linux/`](../deploy/linux/README.md) (systemd) and [`deploy/docker/`](../deploy/docker/README.md).

## Packaging

```powershell
powershell -ExecutionPolicy Bypass -File service/scripts/package.ps1 -IncludeModels -Archive
# production: also -RequireApprovedModels and -CertThumbprint <code-signing cert> (deploy/windows/SIGNING.md)
```

It builds the release binary (`cargo build --release --locked`), checks the Rust licences against
`deny.toml` (`scripts/third-party-licenses.ps1`), and assembles
`service/dist/veriface-<version>-<platform>/` (plus `.zip` / `.tar.gz` and `.sha256` with
`-Archive`):

```text
veriface.exe               (veriface on Linux)
onnxruntime.dll, ...          when fetched (scripts/fetch-onnxruntime.ps1)
config.example.toml
profiles/*.yaml
models/manifest.json          plus README.txt; the model files with -IncludeModels
install/                      deploy/windows/* or deploy/linux/*: install script, config template
licenses/                     ONNX Runtime notices, Apache-2.0, THIRD-PARTY-RUST.txt
sbom/                         models.cdx.json (CycloneDX model BOM), rust-crates.json
NOTICE.txt, BUILDINFO.json    licences summary; version, commit, toolchain, ORT and manifest hashes
logs/
SHA256SUMS                    every file above, `sha256sum -c` format
```

`-RequireApprovedModels` refuses to package unless every model in the manifest is legally
approved with training data cleared for commercial use (`scripts/model-bom.ps1 -RequireApproved`).

`scripts/release-check.ps1` (`.sh`) is the release gate: fmt, clippy, tests, licence policy,
advisories, SBOMs. See its header.

Verify a copied package on Linux with `sha256sum -c SHA256SUMS`, or on Windows with:

```powershell
Get-Content SHA256SUMS | ForEach-Object {
  $hash, $file = $_ -split '  ', 2
  if ((Get-FileHash -Algorithm SHA256 $file).Hash.ToLower() -ne $hash) { "MISMATCH $file" }
}
```

## Performance

Release build, Windows 11, 16 logical cores, `testdata/` fixtures, curl, median of 5 warm
requests, one request at a time, 4 sessions per model, `parallel_stages` on. The machine was
shared with other builds, so each figure is a range over repeated runs.

| Request | Defaults (overrides on) | `intra_op_threads_per_model` off |
|---|---|---|
| `/v1/quality` (819x1024 portrait, with output image) | 149-248 ms | 326-336 ms |
| `/v1/document-portraits` (1600x1200 card) | 240-459 ms | 468-502 ms |
| `/v1/verifications` genuine (embeds 2 faces) | 700-782 ms | 945-1087 ms |
| `/v1/verifications` impostor | 723-818 ms | 993-1065 ms |
| `/v1/verifications` retake (no embedding) | 411-413 ms | 463-550 ms |
| 16 concurrent verifications, 16 cores | 4.5-4.7 requests/s | 3.8-4.5 requests/s |

What keeps a verification fast, all without changing any result (checked on 12 fixture pairs:
identical check statuses, score difference 0):

- `engine.parallel_stages` (default on) runs the selfie analysis, and its probe-only risk
  measurements, alongside the document extraction, and embeds the two faces on two sessions at
  once. A verification uses up to two cores while it runs; stage timings then add up to more than
  `totalMs`.
- `engine.intra_op_threads_per_model` gives AuraFace and the segmenter 2 threads each (one face
  embedding ~390 -> ~250 ms, segmentation ~210 -> ~140 ms). Other models stay at
  `intra_op_threads` (1), which is best for throughput when `max_concurrency` is about the number
  of cores.
- `sessions_per_model` defaults to `max_concurrency` capped at 4 (about 290 MB per set): more
  sessions only cost memory (16 sessions: 4.7 GB, no gain in throughput).

The remaining stages of a verification, critical path first: reference extraction
(`documentMs` ~200-300 ms plus its segmentation and landmarks), embedding (`embedMs`), and on the
probe side the risk measurements (`riskMs`, the document-outline search on the selfie). To
profile one verification stage by stage:
`cargo test --release -p face-server --test profile -- --ignored --nocapture` (set
`PROFILE_OVERRIDES=1` for the 2-thread overrides).

## Operating notes

- **Stage timings** in every result: `queueMs`, `uploadMs`, `decodeMs`, `detectMs`,
  `landmarksMs`, `segmentMs`, `measureMs`, `documentMs`, `riskMs`, `embedMs`, `evaluateMs`,
  `outputMs`
  (only the stages a request ran), plus `totalMs`.
- **Engine errors.** A model that fails to load or run is `503 not-ready`; any other engine
  failure is `500 internal`. Details go to the log only.
- **Load.** At most `max_concurrency` requests hold a processing slot (taken before the upload
  is read, so memory stays bounded); up to `queue_limit` more wait. Anything beyond gets
  `503 overloaded` with `Retry-After: 1`. A request that misses `request_timeout_ms` also gets
  `503 overloaded`; its CPU work cannot be interrupted, so its slot is freed when that work ends.
- **Readiness.** `/health/live` is 200 while the process runs; `/health/ready` is 200 only after
  the engine's warm-up succeeded, and `503 not-ready` (`Retry-After: 5`) before that and while
  shutting down. Image endpoints answer `503 not-ready` until then.
- **Errors** are RFC 9457 `application/problem+json` with the codes in `contracts/openapi.yaml`;
  `instance` is the request id. Unknown paths are `404 not-found` and wrong methods
  `405 method-not-allowed`.
- **Early rejections and uploads.** Some errors are decided before the upload is read (401, 403,
  404, 415, 503, a too-large `Content-Length`). The service then reads and discards the rest of
  the body before answering, so clients that send the whole upload first (most HTTP clients) get
  the problem response rather than a connection reset. Bounds: at most the whole-body limit (a
  larger declared body is not read), at most 64 drains at once, at most the rest of the request
  deadline (at least 2 s). Otherwise, and when the client sent `Expect: 100-continue` (it then
  never sends the body), the response carries `Connection: close`. Counted in
  `face_unread_body_total{outcome}`.
- **Request ids.** `X-Request-Id` (1-128 visible ASCII characters) is echoed in the response
  header, in `requestId` and in the logs; without one, or when it is invalid (too long, not
  visible ASCII, sent twice), the service generates a UUID (contract: never an error).
- **Logs** (JSON by default) have one line per request: request id, method, endpoint, status,
  duration, project, profile, verdict or outcome and decision, failed check ids, stage timings,
  and the problem code. **Never** images, embeddings, scores, check values or face geometry.
- **Metrics** (`GET /metrics`, Prometheus text format, unauthenticated: restrict it on the
  network): `face_requests_total{endpoint,status}`, `face_request_duration_seconds{endpoint}`,
  `face_stage_duration_seconds{stage}`, `face_check_status_total{check,status}`,
  `face_verification_outcome_total{outcome}`, `face_inflight`, `face_queue_depth`,
  `face_unread_body_total{outcome}` (`drained`, `too_large`, `busy`, `timeout`, `error`,
  `not_sent`).
