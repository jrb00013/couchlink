#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
export PATH="$(couchlink_tool_path "${HOME:-}")${PATH:+:$PATH}"
# Preserve reachability overrides exported by run.sh (--local / --online / mesh).
_KEEP_MODE="${COUCHLINK_MODE:-}"
_KEEP_SIGNALING="${COUCHLINK_SIGNALING:-}"
_KEEP_INVITE_SIGNALING="${COUCHLINK_INVITE_SIGNALING:-}"
_KEEP_PUBLIC_IP="${COUCHLINK_PUBLIC_IP:-}"
_KEEP_TURN_URL="${COUCHLINK_TURN_URL:-}"
_KEEP_TURN_EXTERNAL_IP="${COUCHLINK_TURN_EXTERNAL_IP:-}"
_KEEP_MESH="${COUCHLINK_MESH:-}"
_KEEP_MESH_IP="${COUCHLINK_MESH_IP:-}"
_KEEP_MESH_NEED_TURN="${COUCHLINK_MESH_NEED_TURN:-}"
_KEEP_ICE_IPS="${COUCHLINK_ICE_IPS:-}"
# shellcheck disable=SC1091
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"
[[ -n "$_KEEP_MODE" ]] && COUCHLINK_MODE="$_KEEP_MODE"
[[ -n "$_KEEP_SIGNALING" ]] && COUCHLINK_SIGNALING="$_KEEP_SIGNALING"
[[ -n "$_KEEP_INVITE_SIGNALING" ]] && COUCHLINK_INVITE_SIGNALING="$_KEEP_INVITE_SIGNALING"
[[ -n "$_KEEP_PUBLIC_IP" ]] && COUCHLINK_PUBLIC_IP="$_KEEP_PUBLIC_IP"
[[ -n "$_KEEP_MESH" ]] && COUCHLINK_MESH="$_KEEP_MESH"
[[ -n "$_KEEP_MESH_IP" ]] && COUCHLINK_MESH_IP="$_KEEP_MESH_IP"
[[ -n "$_KEEP_MESH_NEED_TURN" ]] && COUCHLINK_MESH_NEED_TURN="$_KEEP_MESH_NEED_TURN"
[[ -n "$_KEEP_ICE_IPS" ]] && COUCHLINK_ICE_IPS="$_KEEP_ICE_IPS"
[[ -n "$_KEEP_TURN_EXTERNAL_IP" ]] && COUCHLINK_TURN_EXTERNAL_IP="$_KEEP_TURN_EXTERNAL_IP"
# Empty TURN in local mode is intentional. Mesh on native Linux skips TURN;
# WSL mesh keeps TURN on the mesh IP (COUCHLINK_MESH_NEED_TURN=1).
if [[ "$_KEEP_MODE" == "local" ]]; then
  unset COUCHLINK_TURN_URL || true
  unset COUCHLINK_INVITE_SIGNALING || true
elif [[ -n "${COUCHLINK_MESH:-}" && "${COUCHLINK_MESH_NEED_TURN:-0}" != "1" ]]; then
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
  # shellcheck disable=SC1091
  source "$ROOT/scripts/ensure-linux-link-libs.sh"
  cargo build --release -p couchlink-host
fi
ARGS=(
  --signaling "${COUCHLINK_SIGNALING:-ws://127.0.0.1:8443/ws}"
  --session-id "$COUCHLINK_SESSION_ID"
  --pin "$COUCHLINK_PIN"
  --preset "${COUCHLINK_PRESET:-1080p60}"
)
[[ -n "${COUCHLINK_INVITE_SIGNALING:-}" ]] && ARGS+=(--invite-signaling "$COUCHLINK_INVITE_SIGNALING")
[[ -n "${COUCHLINK_TURN_URL:-}" ]] && ARGS+=(--turn-url "$COUCHLINK_TURN_URL")
[[ -n "${COUCHLINK_TURN_USER:-}" ]] && ARGS+=(--turn-user "$COUCHLINK_TURN_USER")
[[ -n "${COUCHLINK_TURN_PASS:-}" ]] && ARGS+=(--turn-pass "$COUCHLINK_TURN_PASS")
[[ -n "${COUCHLINK_ICE_IPS:-}" ]] && ARGS+=(--ice-ips "$COUCHLINK_ICE_IPS")
[[ -n "${COUCHLINK_WINDOWS_CAPTURE:-}" ]] && ARGS+=(--windows-capture "$COUCHLINK_WINDOWS_CAPTURE")
# Capture source is handled by ensure-win-capture / win-capture (picker|desktop|window).
exec "$BIN" "${ARGS[@]}"
