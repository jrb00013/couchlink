#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"
exec couchlink-signaling --bind "${COUCHLINK_BIND:-0.0.0.0:8443}" --web-root "$ROOT/web/dist"
