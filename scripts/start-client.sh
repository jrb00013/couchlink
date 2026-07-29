#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
export PATH="$(couchlink_tool_path "${HOME:-}")${PATH:+:$PATH}"
# shellcheck disable=SC1091
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"

# Prefer a full invite link. Missing pieces → couchlink-client prompts interactively.
ARGS=()
if [[ -n "${COUCHLINK_JOIN_URL:-}" ]]; then
  ARGS+=(--join-url "$COUCHLINK_JOIN_URL")
fi
if [[ -n "${COUCHLINK_SESSION_ID:-}" ]]; then
  ARGS+=(--session-id "$COUCHLINK_SESSION_ID")
fi
if [[ -n "${COUCHLINK_PIN:-}" ]]; then
  ARGS+=(--pin "$COUCHLINK_PIN")
fi
if [[ -n "${COUCHLINK_SIGNALING:-}" ]]; then
  ARGS+=(--signaling "$COUCHLINK_SIGNALING")
fi
[[ -n "${COUCHLINK_TURN_URL:-}" ]] && ARGS+=(--turn-url "$COUCHLINK_TURN_URL")
[[ -n "${COUCHLINK_TURN_USER:-}" ]] && ARGS+=(--turn-user "$COUCHLINK_TURN_USER")
[[ -n "${COUCHLINK_TURN_PASS:-}" ]] && ARGS+=(--turn-pass "$COUCHLINK_TURN_PASS")
[[ -n "${COUCHLINK_ICE_IPS:-}" ]] && ARGS+=(--ice-ips "$COUCHLINK_ICE_IPS")

# Same resolution as start-host.sh — workspace binaries before PATH.
BIN="${COUCHLINK_CLIENT_BIN:-$ROOT/target/release/couchlink-client}"
if [[ ! -x "$BIN" ]]; then
  echo "==> building couchlink-client (release)"
  # shellcheck disable=SC1091
  source "$ROOT/scripts/ensure-linux-link-libs.sh"
  cargo build --release -p couchlink-client
fi
exec "$BIN" "${ARGS[@]}"
