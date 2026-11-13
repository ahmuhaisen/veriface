#!/usr/bin/env bash
# The release gate for veriface (bash counterpart of release-check.ps1; see its header for
# what each step does and how to wire it into a pipeline).
#
# Usage: service/scripts/release-check.sh [--strict] [--require-approved-models]
#                                         [--parity-service URL] [--out DIR]
#
# Steps: fmt, clippy, test, licences, deny, advisories, sbom, model-bom, parity. Each is PASS,
# FAIL or NOT RUN. Any FAIL fails the gate (exit 1); with --strict, any NOT RUN does too.
# The licence gate is mandatory: scripts/third-party-licenses.ps1 when pwsh is installed, else
# `cargo deny check licenses`; with neither, the licences step FAILS (never silently passes).
set -uo pipefail

SERVICE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(dirname "$SERVICE_DIR")"
STRICT=0
APPROVED=0
PARITY=""
OUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --strict) STRICT=1; shift ;;
    --require-approved-models) APPROVED=1; shift ;;
    --parity-service) PARITY="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    -h | --help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
OUT="${OUT:-$SERVICE_DIR/dist/release-check-$(date -u +%Y%m%d-%H%M%S)}"
mkdir -p "$OUT/sbom"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$SERVICE_DIR/target-release-check}"
cd "$SERVICE_DIR"

SUMMARY=()
FAILED=0
NOTRUN=0
result() { # step status detail
  SUMMARY+=("$(printf '%-8s %-11s %s' "$2" "$1" "$3")")
  printf '%-8s %-11s %s\n' "$2" "$1" "$3"
  case "$2" in FAIL) FAILED=$((FAILED + 1)) ;; "NOT RUN") NOTRUN=$((NOTRUN + 1)) ;; esac
}
have() { command -v "$1" > /dev/null 2>&1; }
logged() { # step cmd...
  local step="$1"; shift
  "$@" > "$OUT/$step.log" 2>&1
}

echo "veriface release check -> $OUT (CARGO_TARGET_DIR=$CARGO_TARGET_DIR)"

if logged fmt cargo fmt --all --check; then result fmt PASS "formatted"; else result fmt FAIL "see fmt.log"; fi

if logged clippy cargo clippy --workspace --all-targets --locked -- -D warnings; then
  result clippy PASS "no warnings"
else
  result clippy FAIL "see clippy.log"
fi

if logged test cargo test --workspace --locked; then
  totals=$(grep -Eo 'test result: [a-z]+\. [0-9]+ passed; [0-9]+ failed; [0-9]+ ignored' "$OUT/test.log" |
    awk '{p+=$4; f+=$6; i+=$8} END {printf "%d passed, %d failed, %d ignored", p, f, i}')
  result test PASS "$totals"
else
  result test FAIL "see test.log"
fi

if have pwsh; then
  ok=1
  for target in x86_64-pc-windows-msvc x86_64-unknown-linux-gnu; do
    if ! pwsh -NoProfile -File "$SERVICE_DIR/scripts/third-party-licenses.ps1" -Target "$target" \
      -Out "$OUT/THIRD-PARTY-RUST-$target.txt" -Json "$OUT/rust-crates-$target.json" > "$OUT/licences-$target.log" 2>&1; then
      ok=0
    fi
  done
  if [ "$ok" = 1 ]; then
    result licences PASS "every crate allowed by deny.toml (windows + linux targets)"
  else
    result licences FAIL "LICENCE POLICY VIOLATION: $(grep -h '^  - ' "$OUT"/licences-*.log | tr '\n' ';')"
  fi
elif have cargo-deny; then
  if logged licences cargo deny --locked check licenses; then
    result licences PASS "cargo deny check licenses"
  else
    result licences FAIL "LICENCE POLICY VIOLATION: see licences.log"
  fi
else
  result licences FAIL "no licence gate available: install pwsh or cargo-deny"
fi

if have cargo-deny; then
  if logged deny cargo deny --locked check licenses bans sources; then result deny PASS "licenses, bans, sources"; else result deny FAIL "see deny.log"; fi
  if logged advisories cargo deny --locked check advisories; then result advisories PASS "cargo deny check advisories"; else result advisories FAIL "see advisories.log"; fi
else
  result deny "NOT RUN" "cargo-deny not installed"
  if have cargo-audit; then
    if logged advisories cargo audit; then result advisories PASS "cargo audit"; else result advisories FAIL "see advisories.log"; fi
  else
    result advisories "NOT RUN" "neither cargo-deny nor cargo-audit installed"
  fi
fi

if have cargo-cyclonedx; then
  if logged sbom cargo cyclonedx --format json --spec-version 1.5 --no-build-deps; then
    find "$SERVICE_DIR/crates" -name '*.cdx.json' -exec mv {} "$OUT/sbom/" \;
    result sbom PASS "CycloneDX files in sbom/"
  else
    result sbom FAIL "see sbom.log"
  fi
else
  result sbom "NOT RUN" "cargo-cyclonedx not installed"
fi

if have pwsh; then
  args=(-NoProfile -File "$SERVICE_DIR/scripts/model-bom.ps1" -Out "$OUT/sbom/models.cdx.json")
  [ "$APPROVED" = 1 ] && args+=(-RequireApproved)
  if pwsh "${args[@]}" > "$OUT/model-bom.log" 2>&1; then
    n=$(grep -c 'model pack:' "$OUT/model-bom.log" || true)
    if [ "$n" -gt 0 ]; then result model-bom PASS "BOM written; NOT production-approved: $n findings"; else result model-bom PASS "BOM written; every model approved"; fi
  else
    result model-bom FAIL "models not approved for production: see model-bom.log"
  fi
else
  result model-bom "NOT RUN" "pwsh not installed"
fi

if [ -z "$PARITY" ]; then
  result parity "NOT RUN" "no --parity-service"
elif ! have pwsh; then
  result parity "NOT RUN" "pwsh not installed"
else
  pwsh -NoProfile -File "$REPO_DIR/lab/scripts/release-parity.ps1" -Service "$PARITY" -Out "$OUT/parity" > "$OUT/parity.log" 2>&1
  case $? in
    0) result parity PASS "lab parity gate passed" ;;
    3) result parity "NOT RUN" "prerequisites missing: $(tail -n 2 "$OUT/parity.log" | tr '\n' ' ')" ;;
    *) result parity FAIL "parity gate failed; see parity/ and parity.log" ;;
  esac
fi

echo
printf '%s\n' "${SUMMARY[@]}"
gate=PASS
if [ "$FAILED" -gt 0 ] || { [ "$STRICT" = 1 ] && [ "$NOTRUN" -gt 0 ]; }; then gate=FAIL; fi
echo "RELEASE GATE: $gate ($FAILED failed, $NOTRUN not run) -> $OUT"
[ "$gate" = PASS ]
