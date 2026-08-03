#!/usr/bin/env bash
# Point the emulator's Player 2 pad at the couchlink virtual controller.
#
# Without this the friend connects, the pad datachannel opens, and nothing
# happens in-game: RPCS3 ships Player 2 bound to whatever was plugged in when
# the config was written (often a second DualSense that no longer exists), so
# the virtual pad never matches and every button is dropped silently.
#
# Idempotent. Backs up once before the first edit. Never touches Player 1 —
# that is the host's own controller.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

PLAYER="${COUCHLINK_EMU_PLAYER:-2}"

# ViGEm's virtual pad always enumerates through XInput, and the host's real
# DualSense uses the SDL handler — so XInput slot 1 is unambiguous.
case "${COUCHLINK_DS_VHID_BACKEND:-xbox360}" in
  xbox360) HANDLER="XInput"; DEVICE="XInput Pad #1" ;;
  ds4)     HANDLER="SDL";    DEVICE="Wireless Controller 1" ;;
  *)       HANDLER="SDL";    DEVICE="DualSense Wireless Controller 1" ;;
esac
HANDLER="${COUCHLINK_EMU_HANDLER:-$HANDLER}"
DEVICE="${COUCHLINK_EMU_DEVICE:-$DEVICE}"

find_rpcs3_config() {
  if [[ -n "${COUCHLINK_RPCS3_CONFIG:-}" ]]; then
    echo "${COUCHLINK_RPCS3_CONFIG}"
    return
  fi
  local win_user candidates=()
  win_user="$(powershell.exe -NoProfile -Command '$env:USERNAME' 2>/dev/null | tr -d '\r' || true)"
  if [[ -n "$win_user" ]]; then
    candidates+=("/mnt/c/Users/$win_user/RPCS3/config/input_configs/global/Default.yml")
  fi
  candidates+=(
    "$HOME/.config/rpcs3/input_configs/global/Default.yml"
  )
  local c
  for c in "${candidates[@]}"; do
    [[ -f "$c" ]] && { echo "$c"; return; }
  done
}

CONFIG="$(find_rpcs3_config)"
if [[ -z "$CONFIG" ]]; then
  echo "==> RPCS3 pad config not found — skipping (set COUCHLINK_RPCS3_CONFIG)" >&2
  exit 0
fi

# RPCS3 writes this file with CRLF on Windows: match on the stripped line or
# every comparison silently fails on the trailing \r and we report a no-op edit
# as success.
current="$(awk -v p="Player $PLAYER Input:" '
  { line = $0; sub(/\r$/, "", line) }
  line == p { inblock = 1; next }
  line ~ /^Player [0-9]+ Input:/ { inblock = 0 }
  inblock && line ~ /^  Handler:/ { h = line; sub(/^  Handler: /, "", h) }
  inblock && line ~ /^  Device:/ { d = line; sub(/^  Device: /, "", d) }
  END { print h "|" d }
' "$CONFIG")"

want="${HANDLER}|\"${DEVICE}\""
if [[ "$current" == "$want" ]]; then
  echo "==> RPCS3 Player $PLAYER already linked to $HANDLER / $DEVICE"
  exit 0
fi

if [[ ! -f "$CONFIG.couchlink.bak" ]]; then
  cp -f "$CONFIG" "$CONFIG.couchlink.bak"
fi

tmp="$(mktemp)"
awk -v p="Player $PLAYER Input:" -v h="$HANDLER" -v d="$DEVICE" '
  { line = $0; cr = ""; if (sub(/\r$/, "", line)) cr = "\r" }
  line == p { inblock = 1; print line cr; next }
  line ~ /^Player [0-9]+ Input:/ { inblock = 0 }
  inblock && line ~ /^  Handler:/ { print "  Handler: " h cr; next }
  inblock && line ~ /^  Device:/ { print "  Device: \"" d "\"" cr; next }
  { print line cr }
' "$CONFIG" > "$tmp"

# Never leave a truncated pad config behind — RPCS3 silently resets on parse error.
if [[ ! -s "$tmp" ]] || ! grep -q "^Player $PLAYER Input:" "$tmp"; then
  rm -f "$tmp"
  echo "==> refusing to write malformed RPCS3 config — left unchanged" >&2
  exit 1
fi

cat "$tmp" > "$CONFIG"
rm -f "$tmp"
echo "==> RPCS3 Player $PLAYER linked to $HANDLER / $DEVICE (was ${current%%|*})"
echo "    backup: $CONFIG.couchlink.bak"
