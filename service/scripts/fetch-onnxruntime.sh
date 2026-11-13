#!/usr/bin/env bash
# Downloads Microsoft's official ONNX Runtime release that veriface loads at runtime.
#
# The service uses the `ort` crate 2.0.0-rc.13 with `load-dynamic`, which wraps the ONNX Runtime 1.28
# C API. This script fetches the matching official Microsoft build (v1.28.2, CPU only) from
# https://github.com/microsoft/onnxruntime/releases, verifies the archive against the SHA-256 pinned
# below (equal to the digest GitHub publishes for the release asset), and extracts only the runtime
# libraries plus the licence files into
#
#     service/third_party/onnxruntime/1.28.2/<platform>/
#
# `service/third_party/` is git-ignored. Debug symbols and headers are not extracted. The extracted
# libraries are checked against pinned SHA-256 values as well.
#
# Usage: service/scripts/fetch-onnxruntime.sh [linux-x64|win-x64|all] [--force]
#   default platform: linux-x64 on Linux, win-x64 on Windows (Git Bash / MSYS).
# Needs: curl, sha256sum (or shasum), tar; unzip for win-x64.
set -euo pipefail

VERSION="1.28.2"
BASE_URL="https://github.com/microsoft/onnxruntime/releases/download/v${VERSION}"

# Pinned: archive SHA-256 = GitHub release asset digest; library SHA-256 = extracted file.
WIN_ARCHIVE="onnxruntime-win-x64-${VERSION}.zip"
WIN_ARCHIVE_SHA256="c4eedd29489d5feca21866d054638416f3655bf6b18851b3b6b85c8313e95c35"
WIN_LIBS=(
  "lib/onnxruntime.dll=1becbd71adbf49609d33195e29c9214969db3c3de69c81425bebe5a4b69aef97"
  "lib/onnxruntime_providers_shared.dll=2e3a2a3903f1d4b1b15cd6887593ed2f269cd2b24e3d729fead45406ec9a25ac"
)
LINUX_ARCHIVE="onnxruntime-linux-x64-${VERSION}.tgz"
LINUX_ARCHIVE_SHA256="d7209b8751b27b862b0c76332c2e20e203396edb5dab700ecf4bb485cf147415"
LINUX_LIBS=(
  "lib/libonnxruntime.so.${VERSION}=088f24b1fc56714d3efaaeb3ac2ee486a5d7b50ccfbb3dd26fd1a612534a05fb"
  "lib/libonnxruntime_providers_shared.so=086ec1d5388f64153d9c63470d126693db9a182c8ce236d3a1119068471b8a0d"
)
NOTICES=(LICENSE ThirdPartyNotices.txt VERSION_NUMBER GIT_COMMIT_ID)

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) PLATFORM="win-x64" ;;
  *) PLATFORM="linux-x64" ;;
esac
FORCE=0
for arg in "$@"; do
  case "$arg" in
    linux-x64 | win-x64 | all) PLATFORM="$arg" ;;
    --force) FORCE=1 ;;
    -h | --help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

sha256() {
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

SERVICE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="${SERVICE_DIR}/third_party/onnxruntime/${VERSION}"
mkdir -p "$ROOT"

fetch() {
  local plat="$1" archive archive_sha libs
  if [ "$plat" = "win-x64" ]; then
    archive="$WIN_ARCHIVE"; archive_sha="$WIN_ARCHIVE_SHA256"; libs=("${WIN_LIBS[@]}")
  else
    archive="$LINUX_ARCHIVE"; archive_sha="$LINUX_ARCHIVE_SHA256"; libs=("${LINUX_LIBS[@]}")
  fi
  local dest="${ROOT}/${plat}"

  if [ "$FORCE" -eq 0 ]; then
    local present=1 entry member want
    for entry in "${libs[@]}"; do
      member="${entry%%=*}"; want="${entry##*=}"
      if [ ! -f "${dest}/$(basename "$member")" ] || [ "$(sha256 "${dest}/$(basename "$member")")" != "$want" ]; then
        present=0
      fi
    done
    if [ "$present" -eq 1 ]; then
      echo "[$plat] already present and verified in $dest"
      return 0
    fi
  fi

  local url="${BASE_URL}/${archive}" part="${ROOT}/${archive}.part"
  echo "[$plat] downloading $url"
  curl -fsSL --retry 3 -o "$part" "$url"
  local actual
  actual="$(sha256 "$part")"
  if [ "$actual" != "$archive_sha" ]; then
    rm -f "$part"
    echo "[$plat] SHA-256 mismatch for $archive: expected $archive_sha, got $actual" >&2
    exit 1
  fi
  mv -f "$part" "${ROOT}/${archive}"
  echo "[$plat] archive SHA-256 verified: $actual"

  local prefix="${archive%.zip}"; prefix="${prefix%.tgz}"
  local staging="${ROOT}/.staging-${plat}"
  rm -rf "$staging"; mkdir -p "$staging/x"
  local members=() entry
  for entry in "${libs[@]}"; do members+=("${prefix}/${entry%%=*}"); done
  for entry in "${NOTICES[@]}"; do members+=("${prefix}/${entry}"); done
  if [ "$plat" = "win-x64" ]; then
    unzip -o -q "${ROOT}/${archive}" "${members[@]}" -d "$staging/x"
  else
    tar -xzf "${ROOT}/${archive}" -C "$staging/x" "${members[@]}"
  fi
  for entry in "${members[@]}"; do mv -f "$staging/x/$entry" "$staging/"; done
  rm -rf "$staging/x"

  local member want got
  for entry in "${libs[@]}"; do
    member="${entry%%=*}"; want="${entry##*=}"
    got="$(sha256 "$staging/$(basename "$member")")"
    if [ "$got" != "$want" ]; then
      echo "[$plat] SHA-256 mismatch for extracted $member: expected $want, got $got" >&2
      exit 1
    fi
    echo "[$plat] $(basename "$member") SHA-256 $got"
  done
  if [ "$plat" = "linux-x64" ]; then
    # Same symlinks as the upstream archive.
    ln -sf "libonnxruntime.so.${VERSION}" "$staging/libonnxruntime.so.1" 2> /dev/null || true
    ln -sf "libonnxruntime.so.1" "$staging/libonnxruntime.so" 2> /dev/null || true
  fi

  rm -rf "$dest"
  mv "$staging" "$dest"
  rm -f "${ROOT}/${archive}"
  echo "[$plat] ONNX Runtime ${VERSION} ready in $dest"
}

if [ "$PLATFORM" = "all" ]; then
  fetch win-x64
  fetch linux-x64
else
  fetch "$PLATFORM"
fi
