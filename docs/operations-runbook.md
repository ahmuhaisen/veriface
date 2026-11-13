# Operations runbook

For operators who install, run and support the Veriface service (`veriface`). Project teams
calling the service should read the [integration guide](integration-guide.md).

Sources: [`service/README.md`](../service/README.md), [`service/config.example.toml`](../service/config.example.toml),
`service/crates/face-server/src/config.rs`, and the contracts. If this runbook and the code
disagree, the code is right and this runbook has a bug.

Placeholders written as **`[PLANNED: ...]`** describe deployment features that are being built
(Windows bundle and installer, Docker, Linux systemd, TLS, key hot-reload, rate limiting, CI). They
will be filled in when that work lands.

## Contents

1. [What you deploy](#1-what-you-deploy)
2. [Installation](#2-installation)
3. [Configuration reference](#3-configuration-reference)
4. [Startup sequence and readiness](#4-startup-sequence-and-readiness)
5. [Health and metrics](#5-health-and-metrics)
6. [Logs](#6-logs)
7. [API keys and key rotation](#7-api-keys-and-key-rotation)
8. [Common problems and fixes](#8-common-problems-and-fixes)
9. [Sizing guide](#9-sizing-guide)
10. [Backup and what not to back up](#10-backup-and-what-not-to-back-up)
11. [Upgrades and rollback](#11-upgrades-and-rollback)

## 1. What you deploy

| Item | Where (relative to the config file by default) | Notes |
|---|---|---|
| `veriface.exe` (Windows) / `veriface` (Linux) | anywhere | One binary, no installer today |
| Configuration | e.g. `config.toml` | Copy `config.example.toml` |
| Profiles | `profiles/*.yaml` | Thresholds. Validated at startup |
| Models | `models/manifest.json` plus the files it lists | Verified by size and SHA-256 at startup. About 280 MB, most of it AuraFace (`auraface-v1.onnx`, 260 694 219 bytes) |
| ONNX Runtime | `onnxruntime.dll` (Windows) / `libonnxruntime.so.1.28.2` (Linux) | Microsoft's official **1.28.x** build, loaded at runtime |
| ONNX Runtime licences | `licenses/onnxruntime/` | `LICENSE` and `ThirdPartyNotices.txt`, shipped with the library |
| Logs directory | e.g. `logs/` | Needed when running as a Windows service |

The service needs **no network access at runtime** except for its own listening socket. It stores
no images, embeddings or templates.

`service/scripts/package.ps1` assembles a `dist/` folder with all of the above (models only with
`-IncludeModels`) and a `SHA256SUMS` file. Verify a copied package before installing:

```powershell
Get-Content SHA256SUMS | ForEach-Object {
  $hash, $file = $_ -split '  ', 2
  if ((Get-FileHash -Algorithm SHA256 $file).Hash.ToLower() -ne $hash) { "MISMATCH $file" }
}
```

```bash
sha256sum -c SHA256SUMS
```

**`[PLANNED: Windows bundle and installer. Placeholder for the bundle layout, the installer
(MSI or equivalent), its parameters, and silent install.]`**

## 2. Installation

### 2.1 Windows (service)

From an **elevated** prompt, with the files in place (for example `C:\Program Files\Veriface\`):

```powershell
veriface.exe service install --config "C:\Program Files\Veriface\config.toml"   # [--name veriface] [--account ...]
Start-Service veriface
```

| Behaviour | Detail |
|---|---|
| Validation at install | `install` loads ONNX Runtime, verifies the models, and validates the configuration, profiles and keys first. A broken setup is caught before the service exists. The check runs as the installing administrator, so the service account's file permissions are only tested at the first start |
| Config path | Stored as an absolute path |
| Start | Automatic at boot |
| Recovery | The Service Control Manager restarts it after a failure: after 5 s, 30 s, then 120 s; the failure counter resets after a day |
| Account | `NT AUTHORITY\LocalService` by default (least privilege). `--account LocalSystem` is accepted but not recommended; a dedicated virtual account (`--account "NT SERVICE\veriface"`) also works |
| Stop | `Stop-Service veriface` is graceful: in-flight requests finish first |
| Remove | `veriface.exe service uninstall [--name veriface]` stops the service if it runs, then removes it |

The service account must be able to **read** the install directory and **write** the log
directory:

```powershell
icacls "C:\Program Files\Veriface\logs" /grant "NT AUTHORITY\LOCAL SERVICE:(OI)(CI)M"
```

A Windows service has no console. **Set `logging.file`**, or there are no logs. Startup failures
also appear in the Windows event log (Service Control Manager).

### 2.2 Foreground (any platform)

```powershell
veriface run --config C:\veriface\config.toml
```

Ctrl-C (and SIGTERM on Linux) stops it gracefully. On Linux, `run` is the only command.

### 2.3 Linux and containers

The same binary runs in a container with the configuration, profiles, models and
`libonnxruntime.so.1.28.2` mounted or copied in. It stops gracefully on SIGTERM.

**`[PLANNED: Docker image. Placeholder for the image name, tags, volumes, user, health check and
compose example.]`**

**`[PLANNED: Linux systemd unit. Placeholder for the unit file, user, hardening options and log
handling.]`**

### 2.4 TLS

The service speaks **plain HTTP**. Bind it to `127.0.0.1` when the caller is on the same machine.
When traffic leaves the machine, terminate TLS in front of it (a reverse proxy such as IIS ARR,
nginx or a load balancer), and restrict the port with the firewall.

**`[PLANNED: built-in TLS. Placeholder for certificate configuration, protocols and renewal.]`**

## 3. Configuration reference

A TOML file. Relative paths are resolved against the **directory of the configuration file**, so
the service behaves the same from any working directory (a Windows service starts in `System32`).
**Unknown keys are errors.** The service refuses to start on any problem and lists every problem it
found at once.

### 3.1 Top level

| Key | Type | Default | Meaning |
|---|---|---|---|
| `health_requires_key` | bool | `false` | Also require a valid `X-API-Key` on `/health/live` and `/health/ready`. Must appear **before the first `[table]`** (TOML rule). `/metrics` is not affected |

### 3.2 `[server]`

| Key | Type | Default | Valid range | Meaning |
|---|---|---|---|---|
| `bind` | string | `127.0.0.1:8080` | an `ip:port` (not a host name) | Listen address. `0.0.0.0:8080` accepts other hosts |
| `max_concurrency` | integer | `0` | 0-4096 | Requests processed at once. `0` = logical CPUs |
| `queue_limit` | integer | `32` | ≥ 0 | Requests that may wait for a slot; beyond that `503 overloaded`. `0` refuses immediately when all slots are busy |
| `request_timeout_ms` | integer | `30000` | 1-600000 | Deadline for a whole request (queueing, upload, processing). Past it: `503 overloaded` |

### 3.3 `[paths]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `models` | path | **required** | Directory with `manifest.json` and the model files |
| `profiles` | path | **required** | Directory of `*.yaml` profiles. At least one file is required |
| `ort_library` | path | see below | The ONNX Runtime library. When unset: next to the executable, then `third_party/onnxruntime/1.28.2/<platform>/` next to the executable, then next to the config file |

### 3.4 `[[api_keys]]` (one table per integrating project; at least one)

| Key | Type | Default | Meaning |
|---|---|---|---|
| `project` | string | **required** | Name used in logs. Must be non-empty and unique |
| `sha256` | string | **required** | 64 hex characters: the SHA-256 of the key's UTF-8 bytes (either case). **Not the key itself.** Must be unique across entries |
| `profiles` | list of strings | **required** | Profiles this key may use; at least one. `"*"` allows every loaded profile. Each listed profile must be loaded |
| `default_profile` | string | `standard` | Used when a request has no `?profile=`. Must be loaded and allowed for this key |

The example file's placeholder hash is rejected at startup on purpose.

### 3.5 `[engine]`

| Key | Type | Default | Valid range | Meaning |
|---|---|---|---|---|
| `kind` | `pipeline` \| `stub` | `pipeline` | | `stub` loads no models: every check is `undetermined` and verifications end in `review`. **Tests and smoke runs only**; it logs "STUB ENGINE ... Do not deploy." and `engine.version` ends in `+stub` |
| `intra_op_threads` | integer | `1` | 1-64 | ONNX Runtime threads inside one inference. Keep 1 when `max_concurrency` is about the core count |
| `intra_op_threads_per_model` | table `model id = threads` | `"auraface-v1" = 2`, `"mp-selfie-multiclass" = 2` | 1-64 each | Per-model overrides. Writing the table **replaces** the default; an empty table turns the overrides off |
| `sessions_per_model` | integer | `0` | 0-256 | ONNX Runtime sessions per model (each runs one inference at a time). `0` = `max_concurrency`, capped at 4. Each set holds its own weights, about 290 MB |
| `parallel_stages` | bool | `true` | | Run a verification's independent stages concurrently (selfie analysis alongside document extraction; the two embeddings on two sessions, which needs `sessions_per_model` ≥ 2). Same results, lower latency; a verification then uses up to two cores |
| `onnxruntime_sha256` | string | none | 64 hex characters | Optional pin of the ONNX Runtime library file. Startup fails on a mismatch |

### 3.6 `[metrics]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `true` | Serve `GET /metrics` (Prometheus text format, **no authentication**: restrict it on the network) |

### 3.7 `[logging]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `format` | `json` \| `pretty` | `json` | One JSON object per line, or human-readable |
| `level` | string | `info` | `tracing` filter directives, e.g. `info` or `info,face_server=debug` |
| `file` | path | none | Append to this file instead of stdout. **Required for a Windows service.** The directory is created if missing; the service account needs write access |

### 3.8 In progress: keys file, environment overrides, new commands

**`[PLANNED: the deployment work in progress adds the items below. They appeared in the working
tree (service/crates/face-server/src/config.rs, main.rs, healthcheck.rs, deploy/, ops/) while this
runbook was written, and are not yet documented here. Fill in once that work is finished:`**

- **`api_keys_file` (top level): a TOML file holding more `[[api_keys]]` entries, e.g. a mounted
  secret.**
- **`VERIFACE__<TABLE>__<KEY>` environment variables that override config keys (for example
  `VERIFACE__SERVER__BIND`, `VERIFACE__API_KEYS_FILE`; an empty value removes a key;
  `[[api_keys]]` itself cannot be set this way).**
- **`veriface check --config <path>`: validate configuration, ONNX Runtime, models, profiles and
  keys, then exit 0 or 2.**
- **`veriface healthcheck --url <url>` (or `--config`): probe readiness, for container health
  checks.**
- **Windows installer `deploy/windows/Install-Veriface.ps1` (virtual account
  `NT SERVICE\veriface`, ACLs, firewall rule, side-by-side releases), `deploy/windows/SIGNING.md`,
  Docker files in `deploy/docker/`, and Grafana, Prometheus alert and load-test material in
  `ops/`.]`**

### 3.9 Profiles

Profiles are YAML files in `paths.profiles`, following `contracts/profile.schema.json`. They are
owned by the Veriface team (thresholds come from lab calibration). Do not edit shipped profiles
in place; if a project needs different thresholds, ask for a new profile with its own name and
version. Shipped: `standard`, `icao-strict`. Every profile's `verification.model` must be the
loaded face embedder (`auraface-v1`).

## 4. Startup sequence and readiness

1. **Configuration.** Read and validate the TOML file (exit code 2 on any problem, printed to
   stderr before logging starts).
2. **Logging.** Open the log file or stdout (exit code 2 if the file cannot be opened).
3. **ONNX Runtime.** Load the library, check the pinned SHA-256 if set, require the 1.28 C API
   and, when the release's `VERSION_NUMBER` file sits next to it, version 1.28.x. Telemetry is
   switched off. Logs `ONNX Runtime loaded` with path, version and SHA-256.
4. **Models.** Read `models/manifest.json`, verify every model and asset file's size and SHA-256,
   refuse models whose review status is `rejected`, and warn for `pending`
   (`model review status is pending; legal review is required before general availability`).
   Create the sessions.
5. **Profiles.** Load every `*.yaml`, validate them against the schema rules and the check
   catalogue (unknown check ids, a check in the wrong section, `min > max`, an enabled check
   without bounds, required checks that are not enabled, unknown default operating point, and so
   on), and check that each profile's `verification.model` is the loaded embedder.
6. **API keys.** Check every key's profiles and default profile.
7. **Bind** the listen address. Logs `listening` with address, profile count, `max_concurrency`,
   `queue_limit` and engine version.
8. **Warm-up.** One inference per model, in the background. Until it succeeds, `/health/ready` and
   the image endpoints answer `503 not-ready` (`Retry-After: 5`). Logs
   `engine warmed up; ready` with `warm_up_ms`. **If warm-up fails, the service stops.**

Any problem in steps 1-7 stops the service, and every problem found is listed, for example:

```text
veriface: invalid configuration: 2 problems:
  - config.toml: server.bind `localhost` is not an `ip:port` address (e.g. 0.0.0.0:8080)
  - config.toml: api_keys[0] (crm): sha256 must be 64 hex characters (the SHA-256 of the key, not the key)
```

**Shutdown.** On Ctrl-C, SIGTERM or a Windows service stop, the service reports not ready, stops
accepting connections and lets in-flight requests finish, for at most `request_timeout_ms` plus 5
seconds.

## 5. Health and metrics

| Endpoint | Auth | 200 when | Otherwise |
|---|---|---|---|
| `GET /health/live` | none (key if `health_requires_key`) | The process runs | — |
| `GET /health/ready` | none (key if `health_requires_key`) | Warm-up succeeded and not shutting down | `503 not-ready`, `Retry-After: 5` |
| `GET /metrics` | **none** | `metrics.enabled = true` | 404 when disabled |
| `GET /v1/info` | API key | Always | Version, catalogue version, model ids, hashes, review status |

Use `/health/ready` for load-balancer checks and `/health/live` for process supervision.

Metrics (Prometheus text format; labels carry endpoints, statuses, stages, check ids and outcomes
only, never biometric data):

| Metric | Type | Labels | Use |
|---|---|---|---|
| `face_requests_total` | counter | `endpoint`, `status` | Request rate and error rate (alert on 5xx, and on 503 rate) |
| `face_request_duration_seconds` | histogram | `endpoint` | Latency SLOs (buckets 5 ms to 30 s) |
| `face_stage_duration_seconds` | histogram | `stage` (`queue`, `upload`, `decode`, `detect`, `landmarks`, `segment`, `measure`, `document`, `risk`, `embed`, `evaluate`, `output`) | Where time goes. A growing `queue` stage means you need more capacity |
| `face_check_status_total` | counter | `check`, `status` | Which checks fail most; a sudden shift can mean a capture-side problem in a project |
| `face_verification_outcome_total` | counter | `outcome` | Outcome mix. A rise in `review` or `retake` deserves a look |
| `face_inflight` | gauge | | Requests holding a processing slot |
| `face_queue_depth` | gauge | | Requests waiting for a slot. Near `queue_limit` means `503 overloaded` is close |

Suggested alerts: `/health/ready` failing for more than a minute; 5xx (other than 503) above zero;
503 rate above a few percent; p95 latency of `/v1/verifications` above your SLO; `face_queue_depth`
persistently above half of `queue_limit`.

## 6. Logs

| Aspect | Detail |
|---|---|
| Where | stdout, or the file in `logging.file` (appended). For a Windows service, only the file. Startup failures of a Windows service also go to the Windows event log |
| Format | JSON (one object per line) by default, or `pretty` |
| Rotation | None built in. Rotate externally (for example a scheduled task that renames the file while the service is stopped, or a log shipper that truncates). **`[PLANNED: log rotation guidance per platform.]`** |
| Retention | Your policy. Logs contain no biometric data, but they do contain project names and request ids |

**Per-request line** (`"request"`, level `info`; `warn` for 503; `error` for other 5xx):

| Field | Content |
|---|---|
| `request_id` | The client's `X-Request-Id` or a generated UUID |
| `method`, `endpoint`, `status`, `duration_ms` | HTTP facts |
| `project`, `profile` | Which key and profile |
| `verdict` / `outcome`, `decision` | The result |
| `reasons`, `warnings` | Failed check ids (or verification reason codes) |
| `stages_ms` | Stage timings, e.g. `queue=0.0,upload=0.3,decode=4.3,...` |
| `problem`, `detail` | Problem code and its developer detail on errors (detail is cut at 512 characters; it can echo client input such as an unknown profile name) |

At `debug` level, an extra line lists `undetermined` checks with their cause.

**Never logged:** images, embeddings, scores, check values, face geometry, API keys.

**Other lines:** `ONNX Runtime loaded`, model review warnings, `listening`, `engine warmed up;
ready`, `shutdown requested`, `stopped`, engine failures (`engine unavailable`, `engine failure`,
with details that never reach the client), and panics (`panic`, with message and source location;
a panic in one request does not stop the service).

## 7. API keys and key rotation

### 7.1 Issue a key

Generate a random key and its hash. Give the **key** to the integrating project through a secure
channel; put only the **hash** in `[[api_keys]]`.

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

```toml
[[api_keys]]
project = "visitor-portal"
sha256 = "<64 hex characters>"
profiles = ["standard"]
default_profile = "standard"
```

Give each project its own key, and only the profiles it needs. Keys are compared by hash in
constant time.

### 7.2 Rotate a key (overlap, no outage for the project)

The configuration accepts several keys, but **project names and hashes must each be unique**, so
the new key needs a temporary second project name.

1. Generate the new key and hash (7.1).
2. Add a second entry, for example `project = "visitor-portal-2026b"`, with the new hash and the
   same profiles. Keep the old entry.
3. Restart the service (`Restart-Service veriface`; in-flight requests finish first). The
   service is unavailable from the stop until warm-up completes; clients should retry `503` and
   connection errors (integration guide, section 8).
4. The project switches to the new key. Watch the `project` field in the logs until the old name
   no longer appears.
5. Remove the old entry, optionally rename the new one back, and restart again.

For a suspected leak, skip the overlap: replace the hash and restart at once, then issue the new
key to the project.

**`[PLANNED: key hot-reload. Placeholder for reloading [[api_keys]] without a restart.]`**

**`[PLANNED: rate limiting. Placeholder for per-key request limits and their response.]`**

## 8. Common problems and fixes

| Symptom | Likely cause | Fix |
|---|---|---|
| ``model manifest <path>: ...`` | `manifest.json` missing, unreadable or invalid | Restore it from the release package and check `paths.models` |
| ``` `auraface-v1` (<path>): SHA-256 <x> does not match the manifest (<y>) ``` or ``size N bytes, manifest says M`` | A model file is corrupt, truncated (common after an interrupted copy) or not the released one | Re-copy the model files from the release package and verify them with `SHA256SUMS`. **Never edit `manifest.json` to match a file**: the hashes are the integrity control |
| ``cannot read <path>: ...`` for a model | File missing or not readable by the service account | Copy the file; grant read access |
| ``model `<id>` has review status `rejected` and is refused`` | The manifest marks a model as rejected by legal review | Use a model pack without that model; contact the Veriface team |
| ``ONNX Runtime library <path>: ...`` / ``cannot read <path>`` for the library | `onnxruntime.dll` (or `.so`) not found where `paths.ort_library` or the default search points | Copy Microsoft's ONNX Runtime 1.28.x library next to the executable or set `paths.ort_library`. Fetch it with `service/scripts/fetch-onnxruntime.ps1` (or `.sh`) |
| ``version <v> is not supported; ONNX Runtime 1.28.x is required``, or an API version error when loading | Wrong ONNX Runtime version. Typical case: the older `onnxruntime.dll` that Windows ships in `System32` | Point `paths.ort_library` explicitly at the 1.28.x library. Do not use the System32 copy |
| ``SHA-256 <x> does not match the pinned <y>`` | `engine.onnxruntime_sha256` pins a different file | Install the pinned file, or update the pin after verifying the new file |
| Warning `ONNX Runtime version unknown (no matching VERSION_NUMBER next to the library)` | The release's `VERSION_NUMBER` file is not next to the library | Harmless if the library is the official 1.28.x build; copy `VERSION_NUMBER` along to silence it, and pin the hash |
| The binary or `onnxruntime.dll` fails to load on Windows with no clear reason; Event Viewer `Microsoft-Windows-CodeIntegrity/Operational` shows events 3033 or 3077 | **Windows Code Integrity / Smart App Control / WDAC** blocked an unsigned or untrusted file | **Do not disable or bypass Code Integrity.** Use Microsoft's official, Authenticode-signed ONNX Runtime build. For `veriface.exe`, work with your security team on an App Control (WDAC) policy that allows the release, by publisher or hash. Report the event to the Veriface team. **`[PLANNED: code-signed release binaries. Placeholder for the publisher certificate and the policy rule to use.]`** |
| Clients get `503 overloaded`; `face_queue_depth` near `queue_limit` | More requests than `max_concurrency` + `queue_limit`, or requests missing `request_timeout_ms` | Short bursts: clients retry with `Retry-After`. Sustained: add CPU or instances (section 9). Raising `queue_limit` only adds waiting; raising `request_timeout_ms` hides overload |
| `503 overloaded` with detail "did not complete within N ms" | A request missed the deadline (slow machine, very large images, overload) | Check `timings`/`stages_ms`; check image sizes; size the machine. The CPU work continues until it ends, holding its slot |
| `503 not-ready` long after start | Warm-up still running, or a model failed to run (`engine unavailable` in the log) | Check the log. If warm-up failed, the service stopped: see the startup errors |
| ``<file>: ...`` profile errors at startup, e.g. ``verification.requiredProbeChecks: SHARPNESS must be enabled in selfie.checks``, ``an enabled check needs min and/or max``, ``min 5 is greater than max 3``, ``catalogueVersion 2 does not match the service catalogue version 1``, ``profile name `standard` is already defined in ...``, ``no profile files (*.yaml) in ...`` | A profile file is invalid, duplicated, written for another catalogue version, or missing | Restore the shipped profiles from the release package. Custom profiles come from the Veriface team |
| ``profile `<p>`: verification.model `<m>` is not the loaded embedder `auraface-v1` `` | Profile written for another model pack | Use profiles from the same release as the model pack |
| ``api_keys[i] (<project>): profile `<p>` is not loaded`` / ``default_profile ... is not in this key's profiles`` | Key refers to a missing or disallowed profile | Fix the key entry or add the profile |
| ``no [[api_keys]] configured`` / ``sha256 must be 64 hex characters (the SHA-256 of the key, not the key)`` | Missing keys, or the key itself pasted instead of its hash | See section 7 |
| ``cannot listen on <addr>: ...`` | Port in use, or address not on this machine | Free the port or change `server.bind` |
| ``cannot open log file ...`` / ``cannot create log directory ...`` | The service account cannot write the log directory | Grant write access (section 2.1) |
| Windows service starts and stops at once, no log file | `logging.file` not set, or not writable | Set `logging.file`; check the Windows event log (Service Control Manager) |
| Clients get `401 unauthorized` | Wrong key, key sent twice, or hash entered for a different string (for example with a trailing newline) | Recompute the hash from the exact key bytes (`printf '%s'`, not `echo`) |
| Clients report broken uploads instead of a 4xx/503 | The service refused the request before reading the body | Client-side: send `Expect: 100-continue` (integration guide, section 8) |

## 9. Sizing guide

### 9.1 Measured numbers

All figures from [`service/README.md`](../service/README.md#performance) and the measurement notes
in `service/crates/face-server/src/config.rs`: release build, Windows 11, 16 logical cores,
`testdata/` fixtures, median of 5 warm requests, one request at a time unless stated, 4 sessions
per model, `parallel_stages` on. The machine was shared with other builds, so each figure is a
range over repeated runs.

| Request | Defaults (per-model thread overrides on) | Overrides off (all models at 1 thread) |
|---|---|---|
| `/v1/quality` (819x1024, with output image) | 149-248 ms | 326-336 ms |
| `/v1/document-portraits` (1600x1200 card) | 240-459 ms | 468-502 ms |
| `/v1/verifications` genuine | 700-782 ms | 945-1087 ms |
| `/v1/verifications` impostor | 723-818 ms | 993-1065 ms |
| `/v1/verifications` retake (no embedding) | 411-413 ms | 463-550 ms |
| 16 concurrent verifications, 16 cores | 4.5-4.7 requests/s | 3.8-4.5 requests/s |

Memory and sessions (`config.rs`, 16 cores, 16 concurrent verifications):

| `sessions_per_model` | Memory for the model sessions | Throughput |
|---|---|---|
| 4 (the default cap) | about 1.2 GB | 3.9 requests/s |
| 16 | about 4.7 GB | 3.6 requests/s |

About 290 MB per session set, mostly AuraFace. **More sessions cost memory and gave no
throughput**: most of a request's time is spent outside the model sessions.

Rules of thumb drawn from these numbers:

- **Throughput ≈ 0.28-0.29 verifications per second per logical core** on the test machine with
  defaults (4.5-4.7 req/s on 16 cores). Quality-only and document-only calls are cheaper.
- **Latency** of a single verification is about 0.7-0.8 s; `parallel_stages` uses up to two cores
  per verification to achieve that.
- **Memory**: plan about 1.2 GB for the default 4 session sets, plus request buffers (at most
  `max_concurrency` request bodies are held at once; each image is limited by the profile's
  `maxBytes`, 10 MiB in `standard`, and decoded images by `maxPixels`, 40 MP, and a 512 MiB
  decoder allocation limit). Leave headroom for the OS.

### 9.2 Starting points

| Expected peak | Suggested starting point (validate with a load test) |
|---|---|
| Up to ~1 verification/s (supervised desks, a few kiosks) | 4 logical cores, 4 GB RAM, defaults |
| ~2 verifications/s | 8 logical cores, 4-8 GB RAM, defaults |
| ~4-5 verifications/s | 16 logical cores, 8 GB RAM, defaults |
| More | Several instances behind a load balancer (the service is stateless). **`[PLANNED: multi-instance deployment guidance.]`** |

These are extrapolations from one machine. **Measure on your hardware** with your image sizes
before go-live: run concurrent verifications and watch `face_request_duration_seconds`,
`face_stage_duration_seconds{stage="queue"}` and memory.

### 9.3 Tuning knobs

| Knob | When to change |
|---|---|
| `max_concurrency` | Leave at `0` (logical CPUs) unless the machine runs other heavy work; then lower it |
| `queue_limit` | Covers short bursts. Waiting time ≈ queue length × service time / `max_concurrency`; keep it consistent with `request_timeout_ms` |
| `sessions_per_model` | Leave at `0` (cap 4). Lower it to save memory on small machines; raising it did not help |
| `intra_op_threads`, `intra_op_threads_per_model` | Defaults are tuned for throughput. On a lightly loaded machine, more threads cut single-request latency |
| `parallel_stages` | Leave on. Turning it off saves no CPU and raises latency (about 35% per `config.example.toml`) |

## 10. Backup and what not to back up

**There is no biometric data at rest.** The service stores no images, embeddings, templates or
results, and its logs and metrics hold none. There is nothing per person to back up, restore or
delete.

| Back up | Why |
|---|---|
| `config.toml` | Bind address, limits, key hashes. Treat it as sensitive: hashes are not keys, but they identify the integrating projects |
| Custom profiles, if any | Normally they come from the release package |
| The release package itself (binary, ONNX Runtime, profiles, model pack, `SHA256SUMS`) | So you can reinstall or roll back exactly. Keep it with its checksums |

| Do NOT back up or copy | Why |
|---|---|
| Captured images from integrating projects | They never belong on this server. If you find any (for example a debugging copy), delete them and report it |
| The API keys themselves | Only their hashes belong in the config. Keys live with the integrating projects' secret stores |
| Logs as a long-term archive | No biometric data, but follow your retention policy for operational logs |

Model files can be restored from the release package; their SHA-256 values in `manifest.json` are
the integrity reference.

## 11. Upgrades and rollback

See [versioning and upgrades](versioning-and-upgrades.md) for what each version number means.
Short version:

1. Read the release notes (`CHANGELOG.md`). Check whether the binary, API, catalogue, profiles or
   model pack changed. A model pack change means new thresholds.
2. Verify the new package with `SHA256SUMS`.
3. Install side by side (a new directory), point a copy of `config.toml` at it, and run
   `veriface service install` with `--name` for a test instance, or run it in the foreground on
   another port. Fix any startup problems it lists.
4. Switch: stop the old service, start the new one (or move the load balancer).
5. Watch `/health/ready`, the error rate and `face_verification_outcome_total` for a shift.
6. **Rollback:** start the previous release from its untouched directory with its own config,
   profiles and model pack. Never mix a binary with a model pack or profiles from another release
   unless the release notes say they are compatible.

**`[PLANNED: CI-built, signed release artifacts and an upgrade procedure for the installer.]`**
