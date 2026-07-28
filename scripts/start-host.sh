#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"
: "${COUCHLINK_SESSION_ID:?set COUCHLINK_SESSION_ID}"
: "${COUCHLINK_PIN:?set COUCHLINK_PIN}"
exec couchlink-host \
  --signaling "${COUCHLINK_SIGNALING:-ws://127.0.0.1:8443/ws}" \
  --session-id "$COUCHLINK_SESSION_ID" \
  --pin "$COUCHLINK_PIN" \
  --preset "${COUCHLINK_PRESET:-1080p60}"
