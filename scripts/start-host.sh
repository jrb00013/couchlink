#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Preserve reachability overrides exported by run.sh (--local / --online).
_KEEP_MODE="${COUCHLINK_MODE:-}"
_KEEP_SIGNALING="${COUCHLINK_SIGNALING:-}"
_KEEP_PUBLIC_IP="${COUCHLINK_PUBLIC_IP:-}"
_KEEP_TURN_URL="${COUCHLINK_TURN_URL:-}"
# shellcheck disable=SC1091
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"
[[ -n "$_KEEP_MODE" ]] && COUCHLINK_MODE="$_KEEP_MODE"
[[ -n "$_KEEP_SIGNALING" ]] && COUCHLINK_SIGNALING="$_KEEP_SIGNALING"
[[ -n "$_KEEP_PUBLIC_IP" ]] && COUCHLINK_PUBLIC_IP="$_KEEP_PUBLIC_IP"
# Empty TURN URL in local mode is intentional — restore even when blanked via unset.
if [[ "$_KEEP_MODE" == "local" ]]; then
  unset COUCHLINK_TURN_URL || true
elif [[ -n "$_KEEP_TURN_URL" ]]; then
  COUCHLINK_TURN_URL="$_KEEP_TURN_URL"
fi
: "${COUCHLINK_SESSION_ID:?set COUCHLINK_SESSION_ID}"
: "${COUCHLINK_PIN:?set COUCHLINK_PIN}"

# On WSL, bring up Windows DXGI capture before the host connects to it.
"$ROOT/scripts/ensure-win-capture.sh"

# Release only: the BGRA→I420 conversion and scaler are per-pixel Rust loops, and
# a debug build cannot keep up with 1080p60 — it shows up as seconds of video lag.
BIN="${COUCHLINK_HOST_BIN:-$ROOT/target/release/couchlink-host}"
if [[ ! -x "$BIN" ]]; then
  echo "==> building couchlink-host (release)"
  cargo build --release -p couchlink-host
fi
ARGS=(
  --signaling "${COUCHLINK_SIGNALING:-ws://127.0.0.1:8443/ws}"
  --session-id "$COUCHLINK_SESSION_ID"
  --pin "$COUCHLINK_PIN"
  --preset "${COUCHLINK_PRESET:-1080p60}"
)
[[ -n "${COUCHLINK_TURN_URL:-}" ]] && ARGS+=(--turn-url "$COUCHLINK_TURN_URL")
[[ -n "${COUCHLINK_TURN_USER:-}" ]] && ARGS+=(--turn-user "$COUCHLINK_TURN_USER")
[[ -n "${COUCHLINK_TURN_PASS:-}" ]] && ARGS+=(--turn-pass "$COUCHLINK_TURN_PASS")
[[ -n "${COUCHLINK_ICE_IPS:-}" ]] && ARGS+=(--ice-ips "$COUCHLINK_ICE_IPS")
[[ -n "${COUCHLINK_WINDOWS_CAPTURE:-}" ]] && ARGS+=(--windows-capture "$COUCHLINK_WINDOWS_CAPTURE")
# Capture source is handled by ensure-win-capture / win-capture (picker|desktop|window).
exec "$BIN" "${ARGS[@]}"
