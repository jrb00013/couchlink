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
# Capture source/window: allow the parent (run.sh) to force picker even when
# .env.couchlink pins COUCHLINK_CAPTURE_WINDOW=PCSX2 — otherwise the picker
# never appears and win-capture silently waits for a closed emulator.
_KEEP_CAPTURE_SOURCE="${COUCHLINK_CAPTURE_SOURCE:-}"
_HAS_CAPTURE_WINDOW=0
[[ -v COUCHLINK_CAPTURE_WINDOW ]] && _HAS_CAPTURE_WINDOW=1
_KEEP_CAPTURE_WINDOW="${COUCHLINK_CAPTURE_WINDOW:-}"
# `set -a` matters: .env.couchlink assigns without `export`, so without it the
# settings stay shell-local. The host binary still looked correct because its
# preset is passed as an argument, while ensure-win-capture.sh runs as a child
# process, saw nothing, and fell back to its own 1080p60/60fps defaults — the
# viewer got 1080p60 at 18Mbps while every log here said 720p60.
# shellcheck disable=SC1091
set -a
[[ -f "$ROOT/.env.couchlink" ]] && source "$ROOT/.env.couchlink"
set +a
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
if [[ -n "$_KEEP_CAPTURE_SOURCE" ]]; then
  export COUCHLINK_CAPTURE_SOURCE="$_KEEP_CAPTURE_SOURCE"
fi
if [[ "$_HAS_CAPTURE_WINDOW" == "1" ]]; then
  export COUCHLINK_CAPTURE_WINDOW="$_KEEP_CAPTURE_WINDOW"
fi
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

# Picker mode: give the user time to choose a window before the host binary
# starts. Without this, the host races ahead, fails the first Hyper-V connect,
# and (before the ever_connected guard) respawned as Hidden desktop — which
# is why the picker looked like it "never appeared".
if [[ "${COUCHLINK_CAPTURE_SOURCE:-picker}" == "picker" ]] \
  && [[ -z "${COUCHLINK_CAPTURE_WINDOW:-}" ]] \
  && command -v tasklist.exe >/dev/null 2>&1; then
  echo "==> waiting for you to pick a capture window (up to 90s)…"
  _picked=0
  for _ in $(seq 1 90); do
    if tasklist.exe /FI "IMAGENAME eq couchlink-win-capture.exe" 2>/dev/null \
      | grep -qi couchlink-win-capture; then
      echo "==> win-capture is running — continuing host start"
      _picked=1
      break
    fi
    sleep 1
  done
  if [[ "$_picked" != "1" ]]; then
    echo "warning: no capture window picked yet — host will start anyway and attach when you do" >&2
  fi
fi

# Same deal for controller input: without the Windows companion the host has no
# virtual pad and falls back to video-only. Never fatal — video still works.
"$ROOT/scripts/ensure-ds-vhid.sh" || true

# Do not pre-bind emulator pads here. Each remote slot is created and linked
# when that player joins (emulator_pad::apply_on_join), so empty Pad3/4/5
# sections are not written for people who never sat down.

# The host re-runs ensure-ds-vhid + link-emulator-pad on join and again if
# the player reports a different controller family, so it needs to find them
# from a binary living under target/.
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
# Default the host's own listen/connect spec to match what ensure-win-capture.sh
# just told win-capture.exe to dial: a Hyper-V socket (only the port matters
# on this side — see capture/mod.rs). COUCHLINK_CAPTURE_TRANSPORT=tcp opts
# back into the old vEthernet/NAT path.
_windows_capture="${COUCHLINK_WINDOWS_CAPTURE:-}"
if [[ -z "$_windows_capture" ]] && grep -qi microsoft /proc/version 2>/dev/null; then
  if [[ "${COUCHLINK_CAPTURE_TRANSPORT:-hyperv}" == "tcp" ]]; then
    _windows_capture="auto"
  else
    _windows_capture="hyperv:9877"
  fi
fi
[[ -n "$_windows_capture" ]] && ARGS+=(--windows-capture "$_windows_capture")
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
