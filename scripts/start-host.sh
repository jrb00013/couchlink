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
_KEEP_HS_URL="${COUCHLINK_HS_URL:-}"
_KEEP_TS_AUTHKEY="${COUCHLINK_TS_AUTHKEY:-}"
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
[[ -n "$_KEEP_HS_URL" ]] && export COUCHLINK_HS_URL="$_KEEP_HS_URL"
[[ -n "$_KEEP_TS_AUTHKEY" ]] && export COUCHLINK_TS_AUTHKEY="$_KEEP_TS_AUTHKEY"
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

# Same deal for controller input: without the Windows companion the host has no
# virtual pad and falls back to video-only. Never fatal — video still works.
"$ROOT/scripts/ensure-ds-vhid.sh" || true

# Bind the emulator's P2 slot to that virtual pad. RPCS3 keeps whatever device
# was plugged in when its config was written, so a stale binding drops every
# remote button without a single error anywhere.
"$ROOT/scripts/link-emulator-pad.sh" || true

# The host re-runs both of the above when the player reports its controller
# family, so it needs to find them from a binary living under target/.
export COUCHLINK_ROOT="$ROOT"

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
if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
  ARGS+=(--verbose)
fi
if [[ -z "${RUST_LOG:-}" ]]; then
  if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
    export RUST_LOG="couchlink_host=info,webrtc=info"
  else
    export RUST_LOG="warn,couchlink_host=warn,webrtc=error,webrtc_ice=error,hyper=error,tower_http=error"
  fi
fi
exec "$BIN" "${ARGS[@]}"
