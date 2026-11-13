# syntax=docker/dockerfile:1.7
#
# Veriface container image. Build context: the repository root. Two targets:
#
#   slim  binary + ONNX Runtime + profiles; the model pack is mounted at /models (read-only)
#   full  slim + the model pack baked in at /models (about 280 MB more)
#
#   docker build -f deploy/docker/Dockerfile --target full -t veriface:0.1.0-full .
#   docker build -f deploy/docker/Dockerfile --target slim -t veriface:0.1.0-slim .
#
# Prerequisite: the official ONNX Runtime library, fetched and SHA-256-checked on the build host
# (service/scripts/fetch-onnxruntime.sh linux-x64, or fetch-onnxruntime.ps1 -Platform linux-x64).
# The build stage needs crates.io; the image needs no network at runtime.
# deploy/docker/build.ps1 (build.sh) builds both targets and exports them for offline sites.

ARG RUST_IMAGE=rust:1.98.1-bookworm
# glibc 2.36 + libstdc++, no shell, no package manager, uid/gid 65532.
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian12:nonroot
ARG ORT_VERSION=1.28.2

# ------------------------------------------------------------------------------------- build
FROM ${RUST_IMAGE} AS build
WORKDIR /src
COPY contracts/ contracts/
COPY profiles/ profiles/
COPY service/Cargo.toml service/Cargo.lock service/rust-toolchain.toml service/config.example.toml service/
COPY service/crates/ service/crates/
WORKDIR /src/service
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/service/target \
    cargo build --release --locked -p face-server --bin veriface \
 && install -D -m 0755 target/release/veriface /out/veriface

# -------------------------------------------------------------------------------------- slim
FROM ${RUNTIME_IMAGE} AS slim
ARG ORT_VERSION
ARG VERSION=0.0.0
ARG REVISION=unknown
LABEL org.opencontainers.image.title="veriface" \
      org.opencontainers.image.description="Veriface: face quality, document portraits and 1:1 verification over HTTP" \
      org.opencontainers.image.vendor="Ahmad Muhaisen" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.licenses="LicenseRef-Proprietary"

COPY --from=build /out/veriface /app/veriface
COPY service/third_party/onnxruntime/${ORT_VERSION}/linux-x64/libonnxruntime.so.${ORT_VERSION} /app/libonnxruntime.so.${ORT_VERSION}
COPY service/third_party/onnxruntime/${ORT_VERSION}/linux-x64/LICENSE service/third_party/onnxruntime/${ORT_VERSION}/linux-x64/ThirdPartyNotices.txt /app/licenses/onnxruntime/
COPY deploy/licenses/Apache-2.0.txt /app/licenses/
COPY profiles/ /app/profiles/
COPY deploy/docker/config.toml /etc/veriface/config.toml

USER 65532:65532
EXPOSE 8080
# The binary probes itself: the runtime image has no shell or curl.
HEALTHCHECK --interval=10s --timeout=5s --start-period=60s --retries=3 \
    CMD ["/app/veriface", "healthcheck", "--config", "/etc/veriface/config.toml"]
STOPSIGNAL SIGTERM
ENTRYPOINT ["/app/veriface"]
CMD ["run", "--config", "/etc/veriface/config.toml"]

# -------------------------------------------------------------------------------------- full
FROM slim AS full
# Only the files the manifest lists (each is SHA-256-checked again at every start).
COPY models/manifest.json models/mp-canonical-face.json /models/
COPY models/*.onnx /models/
