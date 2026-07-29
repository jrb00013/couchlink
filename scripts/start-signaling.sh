#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Preserve mode from run.sh before .env clobber.
_KEEP_MODE="${COUCHLINK_MODE:-}"
# shellcheck disable=SC1091
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"
[[ -n "$_KEEP_MODE" ]] && COUCHLINK_MODE="$_KEEP_MODE"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-upnp.sh"

MODE="${COUCHLINK_MODE:-local}"

BIND="${COUCHLINK_BIND:-0.0.0.0:8443}"
PORT="${BIND##*:}"

if [[ "$MODE" == "online" ]]; then
  trap 'upnp_close "$PORT" tcp' EXIT
  upnp_open "$PORT" tcp "signaling"
else
  echo "==> local mode — signaling on $BIND (no UPnP)"
fi

BIN="${COUCHLINK_SIGNALING_BIN:-}"
if [[ -z "$BIN" ]]; then
  if [[ -x "$ROOT/target/release/couchlink-signaling" ]]; then
    BIN="$ROOT/target/release/couchlink-signaling"
  elif command -v couchlink-signaling >/dev/null 2>&1; then
    BIN="couchlink-signaling"
  else
    BIN="$ROOT/target/debug/couchlink-signaling"
  fi
fi
if [[ ! -x "$BIN" && "$BIN" != "couchlink-signaling" ]]; then
  echo "==> building couchlink-signaling (release)"
  cargo build --release -p couchlink-signaling
  BIN="$ROOT/target/release/couchlink-signaling"
fi
exec "$BIN" --bind "$BIND" --web-root "$ROOT/web/dist"
