# Veriface on Windows Server

Supported: Windows Server 2019, 2022 and 2025 (x64), with Windows PowerShell 5.1 or PowerShell 7. No other runtime is needed. The bundle carries the binary, Microsoft's ONNX Runtime, the models and the profiles. The service needs no network access at runtime.

## Sizing

| Item | Requirement |
|---|---|
| **CPU** | 4 cores minimum; 8–16 for production. Throughput is about 4–5 verifications/s on 16 cores. |
| **Memory** | About 1.2 GB with the default 4 sessions per model, plus about 50 MB per concurrent request. 4 GB is a comfortable minimum. |
| **Disk** | About 300 MB per release, and 3 releases are kept, so about 1 GB. Logs come on top. |

## The bundle

`veriface-<version>-win-x64.zip` (plus `.zip.sha256`) is built by `service/scripts/package.ps1 -IncludeModels -Archive`.

```text
veriface-<version>-win-x64/
  veriface.exe, onnxruntime.dll, onnxruntime_providers_shared.dll
  models/            manifest.json + 6 ONNX models + canonical face (SHA-256 checked at startup)
  profiles/          standard.yaml, icao-strict.yaml
  install/           Install-Veriface.ps1, config.windows.toml, this README, SIGNING.md
  licenses/          ONNX Runtime notices, Apache-2.0, THIRD-PARTY-RUST.txt
  sbom/              models.cdx.json (CycloneDX model BOM), rust-crates.json
  NOTICE.txt  BUILDINFO.json  config.example.toml  SHA256SUMS
```

Copy the zip and its `.sha256` to the server (offline media is fine).

## Install

From an elevated PowerShell:

```powershell
Expand-Archive .\veriface-1.0.0-win-x64.zip -DestinationPath C:\temp
C:\temp\veriface-1.0.0-win-x64\install\Install-Veriface.ps1 -Action Install `
    -Bundle .\veriface-1.0.0-win-x64.zip -Project crm -FirewallRemoteAddress 10.0.1.10,10.0.1.11
```

The installer:

1. Checks the archive against `.zip.sha256`, every file against `SHA256SUMS`, and the Authenticode signature (`-RequireSignature` makes a valid one mandatory).
2. Unpacks the release to `C:\Program Files\Veriface\releases\<version>` and points the `current` junction at it.
3. Writes `C:\ProgramData\Veriface\config\config.toml` from `config.windows.toml`. It binds `0.0.0.0:8080` (`-Bind` changes that) and logs to `C:\ProgramData\Veriface\logs\veriface.log`. An existing `config.toml` is never overwritten.
4. Generates the first API key for `-Project`, prints it **once**, and stores only its SHA-256 in `config\keys.toml`.
5. **Preflight:** runs `veriface check`, then a trial run of the release on a free loopback port until `/health/ready` answers 200. The trial run includes the model warm-up.
6. Registers the service. It is called `veriface` (display name "Veriface") and is set up as follows:
   - **Account:** the virtual account `NT SERVICE\veriface`. This is a least-privilege account with no password to manage. `-Account` accepts `NT AUTHORITY\LocalService` or a gMSA (`DOMAIN\name$`).
   - **Start:** automatic (delayed start).
   - **Recovery:** restarts after 5 s, 30 s and 120 s, with the failure count reset after a day. Recovery also fires on a non-zero exit, not only on a crash.
   - **SID type:** unrestricted service SID.
7. **ACLs:**
   - `C:\ProgramData\Veriface`: inheritance removed. Administrators and SYSTEM get full control, and the service account gets read only. Normal users get nothing, because the config holds the key hashes.
   - `logs\`: the service account can also write.
   - Install directory: the service account can read and execute.
8. **Firewall:** an inbound rule `Veriface-veriface` for the bound TCP port. It is scoped to the service and to the Domain and Private profiles, and to `-FirewallRemoteAddress` (default `Any`; restrict it to the integrating backends). No rule is created for a loopback bind or with `-NoFirewall`.
9. Starts the service and waits until `/health/ready` answers 200 (`-ReadyTimeoutSec`, default 180).

A second instance (a test instance, for example) needs its own name, folders and port:

```powershell
.\Install-Veriface.ps1 -Action Install -Bundle .\veriface-1.0.0-win-x64.zip -ServiceName veriface-test `
    -InstallRoot 'C:\Program Files\Veriface Test' -DataRoot 'C:\ProgramData\Veriface Test' `
    -Bind 127.0.0.1:18080 -NoFirewall
```

Every change supports `-WhatIf` (show what would happen) and `-Confirm`.

## Check a bundle without installing

```powershell
.\Install-Veriface.ps1 -Action Verify -Bundle .\veriface-1.1.0-win-x64.zip
```

This needs no elevation and changes nothing. It runs the integrity and signature checks, `veriface check`, and a trial run on a spare loopback port. The trial run uses the installed configuration when there is one, otherwise a temporary one.

## Upgrade (side by side, automatic rollback)

```powershell
.\Install-Veriface.ps1 -Action Upgrade -Bundle .\veriface-1.1.0-win-x64.zip
```

1. The new release is checked and unpacked next to the current one.
2. **Preflight** runs while the old version keeps serving. The new binary is tried with the installed configuration on a spare loopback port, with one session per model to save memory. A bad release stops the upgrade here, before any downtime.
3. The service is stopped gracefully (in-flight requests finish), `current` is pointed at the new release, the service is started, and the installer waits for `/health/ready`.
4. If the new version does not become ready, the installer points `current` back at the old release, starts it, and reports the failure.
5. Releases beyond `-KeepReleases` (default 3, always including current and previous) are removed.

Downtime is the stop/start window: a graceful stop plus about 5–15 s of model load and warm-up. For zero downtime, run two servers behind a load balancer and upgrade one at a time. The load balancer should use `/health/ready`, which answers 503 while a server is starting or draining.

Re-installing the same version, or installing an older one, needs `-Force`.

## Roll back

```powershell
.\Install-Veriface.ps1 -Action Rollback                 # to the previous version (state.json)
.\Install-Veriface.ps1 -Action Rollback -Version 1.0.0  # to any kept release
```

## API keys

```powershell
.\Install-Veriface.ps1 -Action NewKey -Project hr-portal -Profiles standard
Restart-Service veriface
```

The key is printed once; `keys.toml` stores only its SHA-256.

**Rotation:**

1. Add a new key for the project under a new project name, e.g. `crm-2026`, and restart.
2. Switch the client to the new key.
3. Delete the old entry from `keys.toml` and restart.

Each entry needs a unique project name.

## Status, logs, removal

```powershell
.\Install-Veriface.ps1 -Action Status
Get-Content 'C:\ProgramData\Veriface\logs\veriface.log' -Tail 20 -Wait
.\Install-Veriface.ps1 -Action Uninstall                # keeps config, keys and logs
.\Install-Veriface.ps1 -Action Uninstall -RemoveData    # deletes them too
```

**Startup failures** appear in three places:

- the service log;
- the System event log (Service Control Manager);
- the output of `& 'C:\Program Files\Veriface\current\veriface.exe' check --config 'C:\ProgramData\Veriface\config\config.toml'`. The `check` command lists every problem it finds.

If Windows Code Integrity blocks the binary (events 3033/3077 in `Microsoft-Windows-CodeIntegrity/Operational`), sign the release (see [SIGNING.md](SIGNING.md)) or allow it through your WDAC policy. Never disable Code Integrity.

## Layout

```text
C:\Program Files\Veriface\
  releases\1.0.0\  releases\1.1.0\     one folder per release, never modified
  current -> releases\1.1.0            junction; the service's ImagePath goes through it
C:\ProgramData\Veriface\
  config\config.toml  config\keys.toml
  logs\veriface.log
  state.json                           current, previous, upgrade history
```
