#!/usr/bin/env bash
# Installs, upgrades, rolls back and removes Veriface as a systemd service.
#
# Usage (as root, except verify/status):
#   install.sh install   [--bundle veriface-<v>-linux-x64.tar.gz] [--bind 0.0.0.0:8080] [--project NAME]
#   install.sh upgrade   --bundle veriface-<v>-linux-x64.tar.gz [--force]
#   install.sh rollback  [--version V]
#   install.sh uninstall [--remove-data]
#   install.sh status
#   install.sh verify    [--bundle ...]        integrity + a trial run on a free loopback port
#   install.sh new-key   --project NAME [--profiles "standard,icao-strict"]
#
# Layout: /opt/veriface/releases/<version>/, /opt/veriface/current -> releases/<version>,
#         /etc/veriface/{config.toml,keys.toml} (root:veriface 0640), journal logs.
# Without --bundle, the release this script is part of (<bundle>/install/install.sh).
# An upgrade is preflighted (check + trial run) while the old version serves; if the new version
# does not become ready after the switch, the previous one is restored automatically.
# Supported: Ubuntu 22.04+, RHEL 9+ (glibc >= 2.34), x86_64, systemd.
set -euo pipefail

ACTION="${1:-}"
[ -n "$ACTION" ] && shift || true
BUNDLE=""
BIND="0.0.0.0:8080"
PROJECT=""
PROFILES="*"
VERSION=""
FORCE=0
REMOVE_DATA=0
READY_TIMEOUT=180
KEEP=3
while [ $# -gt 0 ]; do
  case "$1" in
    --bundle) BUNDLE="$2"; shift 2 ;;
    --bind) BIND="$2"; shift 2 ;;
    --project) PROJECT="$2"; shift 2 ;;
    --profiles) PROFILES="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --force) FORCE=1; shift ;;
    --remove-data) REMOVE_DATA=1; shift ;;
    --ready-timeout) READY_TIMEOUT="$2"; shift 2 ;;
    --keep) KEEP="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

NAME=veriface
ROOT=/opt/veriface
RELEASES="$ROOT/releases"
CURRENT="$ROOT/current"
ETC=/etc/veriface
CONFIG="$ETC/config.toml"
KEYS="$ETC/keys.toml"
STATE="$ETC/state"
UNIT=/etc/systemd/system/$NAME.service
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ORT_LIB=libonnxruntime.so.1.28.2

step() { printf '\033[36m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }
need_root() { [ "$(id -u)" = 0 ] || die "$ACTION needs root (sudo)"; }
sha256() { sha256sum "$1" | cut -d' ' -f1; }

TEMP=""
cleanup() { [ -n "$TEMP" ] && rm -rf "$TEMP"; }
trap cleanup EXIT

# Unpacks (if needed) and verifies a bundle; sets BUNDLE_DIR and BUNDLE_VERSION.
open_bundle() {
  local path="${BUNDLE:-$(dirname "$SCRIPT_DIR")}"
  [ -e "$path" ] || die "bundle not found: $path"
  if [ -f "$path" ]; then
    if [ -f "$path.sha256" ]; then
      (cd "$(dirname "$path")" && sha256sum -c --quiet "$(basename "$path").sha256") || die "archive SHA-256 mismatch"
      echo "    archive SHA-256 matches $(basename "$path").sha256"
    else
      echo "    warning: no $(basename "$path").sha256: the archive's own hash is not checked (SHA256SUMS inside still is)"
    fi
    TEMP="$(mktemp -d)"
    tar -xzf "$path" -C "$TEMP"
    path="$(find "$TEMP" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
  fi
  BUNDLE_DIR="$(cd "$path" && pwd)"
  [ -f "$BUNDLE_DIR/BUILDINFO.json" ] || die "$BUNDLE_DIR is not a veriface bundle (no BUILDINFO.json)"
  grep -q '"platform": *"linux-x64"' "$BUNDLE_DIR/BUILDINFO.json" || die "bundle is not linux-x64"
  BUNDLE_VERSION="$(grep -o '"version": *"[^"]*"' "$BUNDLE_DIR/BUILDINFO.json" | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/')"
  verify_dir "$BUNDLE_DIR"
}

verify_dir() {
  (cd "$1" && sha256sum -c --quiet SHA256SUMS) || die "integrity check failed in $1 (SHA256SUMS)"
  ls "$1"/models/*.onnx > /dev/null 2>&1 || die "no models in $1/models: use a full bundle"
  echo "    SHA256SUMS: $(wc -l < "$1/SHA256SUMS") files verified"
}

current_version() { [ -L "$CURRENT" ] && basename "$(readlink "$CURRENT")" || true; }
previous_version() { [ -f "$STATE" ] && sed -n 's/^previous=//p' "$STATE" || true; }
save_state() { printf 'current=%s\nprevious=%s\nupdated=%s\n' "$1" "$2" "$(date -u +%FT%TZ)" > "$STATE"; chmod 0640 "$STATE"; }

set_current() {
  [ -x "$RELEASES/$1/veriface" ] || die "release $1 is not installed"
  ln -sfn "releases/$1" "$CURRENT.new" && mv -Tf "$CURRENT.new" "$CURRENT"
}

probe_url() {
  local bind
  bind="$(sed -n 's/^[[:space:]]*bind[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$CONFIG" | head -n 1)"
  bind="${bind:-127.0.0.1:8080}"
  bind="${bind/0.0.0.0:/127.0.0.1:}"
  bind="${bind/\[::\]:/[::1]:}"
  echo "http://$bind/health/ready"
}

wait_ready() { # exe url seconds
  local deadline=$((SECONDS + $3))
  while [ $SECONDS -lt $deadline ]; do
    "$1" healthcheck --url "$2" --timeout-ms 2000 > /dev/null 2>&1 && return 0
    sleep 1
  done
  return 1
}

free_port() {
  local port
  for _ in $(seq 1 50); do
    port=$((20000 + RANDOM % 40000))
    if ! (exec 3<> "/dev/tcp/127.0.0.1/$port") 2> /dev/null; then echo "$port"; return; fi
  done
  die "no free loopback port found"
}

# Runs a release once with the installed (or a temporary) configuration on a free loopback port.
preflight() { # release_dir config
  local dir="$1" config="$2" port log pid
  port="$(free_port)"
  log="$(mktemp)"
  echo "    check: $dir/veriface check"
  env VERIFACE__PATHS__MODELS="$dir/models" VERIFACE__PATHS__PROFILES="$dir/profiles" \
    VERIFACE__PATHS__ORT_LIBRARY="$dir/$ORT_LIB" \
    "$dir/veriface" check --config "$config" | sed 's/^/    /'
  echo "    trial run on 127.0.0.1:$port ..."
  env VERIFACE__PATHS__MODELS="$dir/models" VERIFACE__PATHS__PROFILES="$dir/profiles" \
    VERIFACE__PATHS__ORT_LIBRARY="$dir/$ORT_LIB" VERIFACE__SERVER__BIND="127.0.0.1:$port" \
    VERIFACE__SERVER__MAX_CONCURRENCY=1 VERIFACE__ENGINE__SESSIONS_PER_MODEL=1 \
    VERIFACE__HEALTH_REQUIRES_KEY=false \
    "$dir/veriface" run --config "$config" > "$log" 2>&1 &
  pid=$!
  if wait_ready "$dir/veriface" "http://127.0.0.1:$port/health/ready" "$READY_TIMEOUT"; then
    kill "$pid" 2> /dev/null; wait "$pid" 2> /dev/null || true
    rm -f "$log"
    echo "    trial run: ready (warm-up passed)"
  else
    kill "$pid" 2> /dev/null; wait "$pid" 2> /dev/null || true
    tail -n 20 "$log" >&2; rm -f "$log"
    die "preflight: the release did not become ready within ${READY_TIMEOUT}s"
  fi
}

install_release() {
  local target="$RELEASES/$BUNDLE_VERSION"
  if [ -d "$target" ]; then
    if cmp -s "$target/SHA256SUMS" "$BUNDLE_DIR/SHA256SUMS"; then
      echo "    releases/$BUNDLE_VERSION already present and identical"; return
    fi
    [ "$FORCE" = 1 ] || die "releases/$BUNDLE_VERSION exists with different contents (use --force)"
    [ "$(current_version)" != "$BUNDLE_VERSION" ] || die "releases/$BUNDLE_VERSION is current; it cannot be replaced in place"
    rm -rf "$target"
  fi
  mkdir -p "$RELEASES"
  cp -a "$BUNDLE_DIR" "$target.tmp" && mv "$target.tmp" "$target"
  chown -R root:root "$target"
  chmod -R u=rwX,go=rX "$target"
  verify_dir "$target"
}

new_key_entry() { # project -> prints key, appends to $KEYS
  local project="$1" key sha list
  [[ "$project" =~ ^[A-Za-z0-9._\ -]{1,64}$ ]] || die "project name must be 1-64 letters, digits, space, . _ -"
  key="$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=\n')"
  sha="$(printf '%s' "$key" | sha256sum | cut -d' ' -f1)"
  list="$(printf '%s' "$PROFILES" | awk -F, '{for (i = 1; i <= NF; i++) printf "%s\"%s\"", (i > 1 ? ", " : ""), $i}')"
  if [ -f "$KEYS" ] && grep -q "^project = \"$project\"$" "$KEYS"; then die "keys.toml already has project '$project'"; fi
  printf '\n# created %s\n[[api_keys]]\nproject = "%s"\nsha256 = "%s"\nprofiles = [%s]\n' "$(date -u +%F)" "$project" "$sha" "$list" >> "$KEYS"
  chown root:$NAME "$KEYS"; chmod 0640 "$KEYS"
  NEW_KEY="$key"
}

prune() {
  local keep_current="$1" keep_previous="$2" kept=0
  for dir in $(ls -1dt "$RELEASES"/*/ 2> /dev/null); do
    v="$(basename "$dir")"
    if [ "$v" = "$keep_current" ] || [ "$v" = "$keep_previous" ]; then continue; fi
    kept=$((kept + 1))
    if [ $((kept + 2)) -gt "$KEEP" ]; then rm -rf "$dir"; echo "    removed old release $v"; fi
  done
}

case "$ACTION" in
  verify)
    open_bundle
    step "Verifying veriface $BUNDLE_VERSION"
    config="$CONFIG"
    if [ ! -r "$config" ]; then
      tmpcfg="$(mktemp)"; config="$tmpcfg"
      sha="$(head -c 32 /dev/urandom | sha256sum | cut -d' ' -f1)"
      printf '[paths]\nmodels = "m"\nprofiles = "p"\n[[api_keys]]\nproject = "verify"\nsha256 = "%s"\nprofiles = ["*"]\n' "$sha" > "$config"
      echo "    no installed configuration: using a temporary one"
    fi
    preflight "$BUNDLE_DIR" "$config"
    [ -n "${tmpcfg:-}" ] && rm -f "$tmpcfg"
    echo "Bundle $BUNDLE_VERSION verified."
    ;;

  install)
    need_root
    systemctl cat "$NAME" > /dev/null 2>&1 && die "$NAME is already installed: use upgrade"
    open_bundle
    step "Installing veriface $BUNDLE_VERSION"
    id -u "$NAME" > /dev/null 2>&1 || useradd --system --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin "$NAME"
    mkdir -p "$ROOT" "$ETC"
    chmod 0755 "$ROOT"; chown root:$NAME "$ETC"; chmod 0750 "$ETC"
    install_release
    if [ ! -f "$CONFIG" ]; then
      sed "s|{{BIND}}|$BIND|" "$BUNDLE_DIR/install/config.linux.toml" > "$CONFIG"
      chown root:$NAME "$CONFIG"; chmod 0640 "$CONFIG"
    else
      echo "    keeping the existing $CONFIG"
    fi
    NEW_KEY=""
    if [ ! -f "$KEYS" ]; then
      printf '# Veriface API keys: only SHA-256 hashes. Add one with: install.sh new-key --project NAME\n' > "$KEYS"
      new_key_entry "${PROJECT:-default}"
    fi
    step "Preflight"
    preflight "$RELEASES/$BUNDLE_VERSION" "$CONFIG"
    set_current "$BUNDLE_VERSION"
    install -m 0644 "$BUNDLE_DIR/install/veriface.service" "$UNIT"
    systemctl daemon-reload
    systemctl enable --now "$NAME"
    save_state "$BUNDLE_VERSION" ""
    wait_ready "$CURRENT/veriface" "$(probe_url)" "$READY_TIMEOUT" || { journalctl -u "$NAME" -n 30 --no-pager; die "not ready within ${READY_TIMEOUT}s"; }
    echo "veriface $BUNDLE_VERSION is ready."
    if [ -n "$NEW_KEY" ]; then
      printf '\nAPI key (shown ONCE; only its SHA-256 is stored in %s):\n    %s\n' "$KEYS" "$NEW_KEY"
    fi
    ;;

  upgrade)
    need_root
    from="$(current_version)"
    [ -n "$from" ] || die "not installed: use install"
    open_bundle
    to="$BUNDLE_VERSION"
    step "Upgrading from $from to $to"
    if [ "$to" = "$from" ] && [ "$FORCE" != 1 ]; then die "$to is already current (--force to reinstall)"; fi
    if [ "$FORCE" != 1 ] && [ "$(printf '%s\n%s\n' "$from" "$to" | sort -V | head -n 1)" = "$to" ] && [ "$to" != "$from" ]; then
      die "$to is older than $from: use rollback, or --force"
    fi
    install_release
    step "Preflight of $to while $from keeps serving"
    preflight "$RELEASES/$to" "$CONFIG"
    step "Switching to $to"
    install -m 0644 "$RELEASES/$to/install/veriface.service" "$UNIT"
    systemctl daemon-reload
    systemctl stop "$NAME"
    set_current "$to"
    systemctl start "$NAME" || true
    if wait_ready "$CURRENT/veriface" "$(probe_url)" "$READY_TIMEOUT"; then
      save_state "$to" "$from"
      prune "$to" "$from"
      echo "Upgrade to $to done. Roll back with: install.sh rollback"
    else
      journalctl -u "$NAME" -n 30 --no-pager || true
      echo "veriface $to did not become ready: rolling back to $from" >&2
      systemctl stop "$NAME" || true
      set_current "$from"
      install -m 0644 "$RELEASES/$from/install/veriface.service" "$UNIT"
      systemctl daemon-reload
      systemctl start "$NAME"
      wait_ready "$CURRENT/veriface" "$(probe_url)" "$READY_TIMEOUT" || die "rollback to $from did not become ready either: investigate now"
      save_state "$from" "$to"
      die "upgrade to $to failed; $from is serving again"
    fi
    ;;

  rollback)
    need_root
    from="$(current_version)"
    to="${VERSION:-$(previous_version)}"
    [ -n "$to" ] || die "no previous version recorded; use --version"
    [ "$to" != "$from" ] || die "$to is already current"
    step "Rolling back from $from to $to"
    verify_dir "$RELEASES/$to"
    systemctl stop "$NAME"
    set_current "$to"
    install -m 0644 "$RELEASES/$to/install/veriface.service" "$UNIT"
    systemctl daemon-reload
    systemctl start "$NAME"
    wait_ready "$CURRENT/veriface" "$(probe_url)" "$READY_TIMEOUT" || die "$to did not become ready"
    save_state "$to" "$from"
    echo "Rolled back to $to."
    ;;

  uninstall)
    need_root
    step "Removing veriface"
    systemctl disable --now "$NAME" 2> /dev/null || true
    rm -f "$UNIT"; systemctl daemon-reload
    rm -rf "$ROOT"
    if [ "$REMOVE_DATA" = 1 ]; then rm -rf "$ETC"; userdel "$NAME" 2> /dev/null || true; else echo "    kept $ETC (config, keys); --remove-data deletes it"; fi
    echo "Removed."
    ;;

  status)
    systemctl status "$NAME" --no-pager -n 0 2> /dev/null || echo "service: not installed"
    echo "current:  $(current_version)"
    echo "previous: $(previous_version)"
    echo "releases: $(ls -1 "$RELEASES" 2> /dev/null | tr '\n' ' ')"
    if [ -x "$CURRENT/veriface" ] && [ -r "$CONFIG" ]; then "$CURRENT/veriface" healthcheck --url "$(probe_url)" || true; fi
    ;;

  new-key)
    need_root
    [ -n "$PROJECT" ] || die "--project is required"
    [ -d "$ETC" ] || die "$ETC does not exist: install first"
    new_key_entry "$PROJECT"
    printf 'Added a key for %s to %s. Apply with: systemctl restart %s\nAPI key (shown ONCE):\n    %s\n' "$PROJECT" "$KEYS" "$NAME" "$NEW_KEY"
    ;;

  *)
    sed -n '2,19p' "$0"; exit 2 ;;
esac
