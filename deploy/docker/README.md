# Veriface in Docker

There are two images, built from the same [`Dockerfile`](Dockerfile). The build context is the repository root.

| Image | Contents | Size | Use when |
|---|---|---|---|
| `veriface:<v>-full` | Binary, ONNX Runtime, profiles, **models baked in** (`/models`) | ~330 MB | Simplest; one artifact per release |
| `veriface:<v>-slim` | Binary, ONNX Runtime, profiles; **models mounted** read-only at `/models` | ~50 MB | The model pack is versioned and distributed separately, or shared by several containers |

Both images share these properties:

- **Base:** `gcr.io/distroless/cc-debian12:nonroot`, with glibc 2.36 and libstdc++. There is no shell and no package manager.
- **User and filesystem:** the service runs as uid/gid 65532 (non-root) and works with a read-only root filesystem.
- **Network:** none is needed at runtime.
- **Health:** a `HEALTHCHECK` runs `veriface healthcheck`, because the image has no curl.
- **Configuration:** `/etc/veriface/config.toml` ([`config.toml`](config.toml)). It binds `0.0.0.0:8080`, logs JSON to stdout, and reads API keys from the secret `/run/secrets/veriface-keys`.

## Build

You need a build host with Docker/BuildKit and internet access (for crates.io). First fetch the official ONNX Runtime library:

```bash
service/scripts/fetch-onnxruntime.sh linux-x64            # SHA-256-pinned, into service/third_party/
deploy/docker/build.sh                                    # both images + offline tarballs in service/dist/
```

On Windows with Docker Desktop (Linux containers):

```powershell
powershell -ExecutionPolicy Bypass -File service/scripts/fetch-onnxruntime.ps1 -Platform linux-x64
powershell -ExecutionPolicy Bypass -File deploy/docker/build.ps1
```

The build scripts write these files to `service/dist/`. Each one has a `.sha256` file next to it.

| File | Contents |
|---|---|
| `veriface-<v>-full.docker.tar.gz` | The full image, for `docker load` |
| `veriface-<v>-slim.docker.tar.gz` | The slim image, for `docker load` |
| `veriface-<v>-models.tar.gz` | The model pack for the slim image |

To build a single target:

```bash
docker build -f deploy/docker/Dockerfile --target full --build-arg VERSION=1.0.0 -t veriface:1.0.0-full .
```

The `RUNTIME_IMAGE` build argument swaps the base, for example to `debian:bookworm-slim` when you need a shell for debugging. Keep the non-root `USER`.

## Run (offline site)

```bash
sha256sum -c veriface-1.0.0-full.docker.tar.gz.sha256
docker load -i veriface-1.0.0-full.docker.tar.gz

cd deploy/docker                    # or any folder holding docker-compose.yml
cp keys.example.toml keys.toml      # put your key hash(es) in it; readable by uid 65532
VERIFACE_VERSION=1.0.0 docker compose up -d
docker compose ps                   # STATUS shows (healthy) after the warm-up (~5-15 s)
```

[`docker-compose.yml`](docker-compose.yml) sets the following:

- `read_only: true` with a small `tmpfs` at `/tmp`.
- `cap_drop: [ALL]`, `no-new-privileges`, and user 65532.
- `pids_limit`.
- CPU and memory limits: 4 CPUs and 4 GB by default, sized for about 1.2 GB of model sessions plus requests.
- `stop_grace_period: 40s`, so in-flight requests finish.
- Rotated json-file logs.
- `server.max_concurrency` set to the CPU limit.

The port is published on **loopback only** (`127.0.0.1:8080`). Change it to `8080:8080` only behind a firewall that admits just the integrating backends. Never expose `/metrics` publicly.

### Slim image + model pack

```bash
docker load -i veriface-1.0.0-slim.docker.tar.gz
mkdir -p /srv/veriface && tar -xzf veriface-1.0.0-models.tar.gz -C /srv/veriface    # -> /srv/veriface/models
VERIFACE_VERSION=1.0.0 VERIFACE_MODELS=/srv/veriface/models \
  docker compose -f docker-compose.yml -f docker-compose.slim.yml up -d
```

The lab's versioned pack (`veriface-models-<v>.zip`, from `python -m facelab models pack`) works the same way: mount its `models/` folder.

At every start, the service checks each model file against `manifest.json` (SHA-256). A wrong or partial pack never serves; the container exits with the list of problems.

### Plain `docker run`

```bash
docker run -d --name veriface --read-only --tmpfs /tmp --cap-drop ALL \
  --security-opt no-new-privileges -p 127.0.0.1:8080:8080 --cpus 4 --memory 4g \
  -e VERIFACE__SERVER__MAX_CONCURRENCY=4 \
  -v "$PWD/keys.toml:/run/secrets/veriface-keys:ro" \
  veriface:1.0.0-full
```

## Configuration

Any key can be changed without rebuilding:

- **Environment variables.** Use the form `VERIFACE__<TABLE>__<KEY>=value`, for example:
  - `VERIFACE__LOGGING__LEVEL=info,face_server=debug`
  - `VERIFACE__SERVER__QUEUE_LIMIT=64`
  - `VERIFACE__HEALTH_REQUIRES_KEY=true`
- **A mounted file.** Mount your own `config.toml` over `/etc/veriface/config.toml`.

**API keys:**

- Keys come only from a file (`api_keys_file`), never from environment variables, which leak through `docker inspect`. The file holds only SHA-256 hashes.
- To rotate a key: add the new entry, restart, switch the clients, remove the old entry, and restart again.

With `health_requires_key = true`, set `VERIFACE_HEALTHCHECK_KEY` in the container environment so the `HEALTHCHECK` can authenticate.

## Kubernetes notes

The image fits a restricted Pod Security profile as-is. Set the following:

- `runAsNonRoot: true`
- `readOnlyRootFilesystem: true`
- `allowPrivilegeEscalation: false`
- `capabilities.drop: [ALL]`
- an `emptyDir` at `/tmp`
- keys from a Secret mounted at `/run/secrets/veriface-keys`

Use these probes:

- **liveness:** `httpGet /health/live`
- **readiness:** `httpGet /health/ready`
- **startup:** `httpGet /health/ready`, `failureThreshold: 30`, `periodSeconds: 2`

Set `terminationGracePeriodSeconds: 40`. For the slim image, mount the model pack from a read-only volume, for example a PVC filled once, or an image volume.

## Verification status

**Not verified on this development machine.** Docker Desktop is installed, but its Linux engine cannot start without WSL2. The files are complete, and the configuration was validated with `veriface check` against the same paths (via environment overrides). An image build, `docker compose up`, the HEALTHCHECK and the read-only run are still to be done, on a host with a working Linux engine.
