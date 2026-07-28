#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-upnp.sh"

BIND="${COUCHLINK_BIND:-0.0.0.0:8443}"
PORT="${BIND##*:}"
trap 'upnp_close "$PORT" tcp' EXIT
upnp_open "$PORT" tcp "signaling"

couchlink-signaling --bind "$BIND" --web-root "$ROOT/web/dist"
