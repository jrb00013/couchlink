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

BIN="${COUCHLINK_SIGNALING_BIN:-$ROOT/target/debug/couchlink-signaling}"
command -v couchlink-signaling >/dev/null 2>&1 && BIN="couchlink-signaling"
exec "$BIN" --bind "$BIND" --web-root "$ROOT/web/dist"
