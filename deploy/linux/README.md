# Veriface on Linux (systemd)

**Supported:**

- Ubuntu 22.04 or later, RHEL / Rocky / Alma 9 or later, or any x86_64 distribution with glibc 2.34 or later and systemd.
- The service needs no network access at runtime.

**Sizing:** see [`deploy/windows/README.md`](../windows/README.md#sizing). The figures are the same on Linux.

## The bundle

Build `veriface-<version>-linux-x64.tar.gz` (and its `.sha256`) on a Linux build host with PowerShell 7:

```bash
service/scripts/fetch-onnxruntime.sh linux-x64
pwsh service/scripts/package.ps1 -IncludeModels -Archive         # or -ModelPack veriface-models-<v>.zip
```

It has the same layout as the Windows bundle. The differences are:

- the binary is `veriface` (mode 0755);
- the ONNX Runtime library is `libonnxruntime.so.1.28.2`;
- `install/` holds `install.sh`, `veriface.service` and `config.linux.toml`.

## Install

```bash
tar -xzf veriface-1.0.0-linux-x64.tar.gz
sudo veriface-1.0.0-linux-x64/install/install.sh install \
    --bundle veriface-1.0.0-linux-x64.tar.gz --project crm --bind 0.0.0.0:8080
```

The script does the following:

1. Checks the archive against its `.sha256` and every file against `SHA256SUMS`.
2. Creates the system user `veriface`, which has no login and no home directory.
3. Unpacks the release to `/opt/veriface/releases/<version>`, owned by root and read-only to the service.
4. Writes `/etc/veriface/config.toml` from `config.linux.toml`. An existing file is never overwritten.
5. Creates the first API key in `/etc/veriface/keys.toml` and prints the key once. Both files are `root:veriface 0640`.
6. Runs the preflight:
   - `veriface check`;
   - a trial run of the release on a free loopback port until `/health/ready` answers 200.
7. Points `/opt/veriface/current` at the release.
8. Installs `veriface.service`, then enables and starts it.
9. Waits until the service is ready.

Logs go to the journal: `journalctl -u veriface -f` (JSON lines).

## Upgrade, roll back, remove

| Command | What it does |
|---|---|
| `sudo install/install.sh upgrade --bundle veriface-1.1.0-linux-x64.tar.gz` | Upgrade side by side (see below). |
| `sudo install/install.sh rollback` | Return to the previous version. Add `--version 1.0.0` for any kept release. |
| `sudo install/install.sh status` | Show the service state and versions. |
| `sudo install/install.sh new-key --project hr-portal --profiles standard` | Add an API key, then `systemctl restart veriface`. |
| `sudo install/install.sh uninstall` | Remove the service and releases. Config and keys are kept unless `--remove-data` is given. |
| `install/install.sh verify --bundle <tar.gz>` | Check a bundle with a trial run, without installing. Needs no root. |

An upgrade runs in this order:

1. The new release is unpacked next to the old one.
2. It is preflighted with the installed configuration while the old version keeps serving.
3. The service is stopped gracefully, so in-flight requests finish.
4. The `current` symlink is moved to the new release.
5. The service is started.
6. If the new version does not become ready, the script restores the previous one automatically.

Releases beyond `--keep` (default 3) are removed. Current and previous are always kept.

## Hardening

`veriface.service` runs the service as the unprivileged `veriface` user with:

- `NoNewPrivileges`, an empty capability set, and `ProtectSystem=strict`, so the whole filesystem is read-only to the service.
- `ProtectHome`, `PrivateTmp`, `PrivateDevices`, `PrivateIPC`.
- Kernel, clock, hostname and cgroup protection.
- `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX`, `RestrictNamespaces`, `RestrictSUIDSGID`, `LockPersonality`, `MemoryDenyWriteExecute`.
- A `@system-service` syscall allow-list, with the privileged, mount, debug, raw-io, reboot and swap groups removed.

Check the result with `systemd-analyze security veriface`.

To accept connections only from the integrating backends, uncomment `IPAddressDeny=any` and `IPAddressAllow=...` in the unit, or use the host firewall:

```bash
sudo firewall-cmd --permanent --add-rich-rule='rule family=ipv4 source address=10.0.1.0/24 port port=8080 protocol=tcp accept'
sudo ufw allow from 10.0.1.0/24 to any port 8080 proto tcp
```

`/metrics` has no authentication. Expose it only to the monitoring network.

## Verification status

The unit and `install.sh` have been syntax-checked only (`bash -n`). They have not yet run on a Linux host. In particular, `MemoryDenyWriteExecute` and the syscall filter must be confirmed against ONNX Runtime 1.28 on a real host:

```bash
sudo install/install.sh install --bundle ... && systemd-analyze security veriface
```

If ONNX Runtime trips the filter, the warm-up fails with a clear error and the journal shows `SIGSYS`/`EPERM`. Remove the offending line and report it.
