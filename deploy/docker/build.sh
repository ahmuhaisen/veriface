#!/usr/bin/env bash
# Builds the veriface images (slim and full) and exports them for offline sites.
#
# Usage: deploy/docker/build.sh [--version X.Y.Z] [--out DIR] [--no-export]
#
# Output (default DIR = service/dist):
#   veriface-<version>-slim.docker.tar.gz    docker load -i ...
#   veriface-<version>-full.docker.tar.gz
#   veriface-<version>-models.tar.gz         the model pack for the slim image
#   *.sha256
# Needs: docker with BuildKit (buildx), the ONNX Runtime linux-x64 library fetched by
# service/scripts/fetch-onnxruntime.sh linux-x64, and the model files in models/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION=""
OUT="$ROOT/service/dist"
EXPORT=1
while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --no-export) EXPORT=0; shift ;;
    -h | --help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [ -z "$VERSION" ]; then
  VERSION="$(awk '/^\[workspace.package\]/{p=1} p && /^version *=/{gsub(/[" ]/,"",$0); sub(/version=/,"",$0); print; exit}' "$ROOT/service/Cargo.toml")"
fi
ORT_VERSION="1.28.2"
ORT_LIB="$ROOT/service/third_party/onnxruntime/$ORT_VERSION/linux-x64/libonnxruntime.so.$ORT_VERSION"
[ -f "$ORT_LIB" ] || { echo "missing $ORT_LIB: run service/scripts/fetch-onnxruntime.sh linux-x64" >&2; exit 1; }
[ -f "$ROOT/models/manifest.json" ] || { echo "missing models/manifest.json" >&2; exit 1; }

REVISION="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi; }

for target in slim full; do
  tag="veriface:$VERSION-$target"
  echo "==> building $tag"
  docker build -f "$ROOT/deploy/docker/Dockerfile" --target "$target" \
    --build-arg VERSION="$VERSION" --build-arg REVISION="$REVISION" \
    -t "$tag" "$ROOT"
done

if [ "$EXPORT" = 1 ]; then
  mkdir -p "$OUT"
  for target in slim full; do
    file="$OUT/veriface-$VERSION-$target.docker.tar.gz"
    echo "==> exporting $file"
    docker save "veriface:$VERSION-$target" | gzip -6 > "$file"
    (cd "$OUT" && sha256 "$(basename "$file")" > "$(basename "$file").sha256")
  done
  # The model pack for the slim image: the same files the full image bakes in.
  pack="$OUT/veriface-$VERSION-models.tar.gz"
  (cd "$ROOT" && tar -czf "$pack" models/manifest.json models/mp-canonical-face.json models/*.onnx)
  (cd "$OUT" && sha256 "$(basename "$pack")" > "$(basename "$pack").sha256")
  echo "==> done"
  ls -l "$OUT"/veriface-"$VERSION"-*
fi
