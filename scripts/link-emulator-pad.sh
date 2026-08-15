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
# DualSense uses the SDL handler — so XInput slot 1 is unambiguous for the
# first remote player. The companion now plugs in one target per connected
# player, in join order (PLAYER-1 = that slot's 1-based index among remote
# pads), so a 2nd/3rd player needs a distinct device name too — otherwise
# every remote slot binds RPCS3's Player N to the SAME device as slot 1.
# NOTE: the numeric suffix RPCS3 assigns a repeated device name has not been
# verified live for 2+ simultaneous virtual pads — confirm this against
# RPCS3's own Controller Settings before trusting it past the first slot.
pad_index=$((PLAYER - 1))
case "${COUCHLINK_DS_VHID_BACKEND:-xbox360}" in
  xbox360) HANDLER="XInput"; DEVICE="XInput Pad #${pad_index}" ;;
  ds4)     HANDLER="SDL";    DEVICE="Wireless Controller ${pad_index}" ;;
  *)       HANDLER="SDL";    DEVICE="DualSense Wireless Controller ${pad_index}" ;;
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

link_rpcs3() {
local CONFIG current want tmp
CONFIG="$(find_rpcs3_config)"
if [[ -z "$CONFIG" ]]; then
  echo "==> RPCS3 pad config not found — skipping (set COUCHLINK_RPCS3_CONFIG)" >&2
  return 0
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
  return 0
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
  return 1
fi

cat "$tmp" > "$CONFIG"
rm -f "$tmp"
echo "==> RPCS3 Player $PLAYER linked to $HANDLER / $DEVICE (was ${current%%|*})"
echo "    backup: $CONFIG.couchlink.bak"

}

link_rpcs3 || true

# ---------------------------------------------------------------- PCSX2 -----
# PCSX2 binds per button rather than by device name, so the whole Pad block is
# rewritten. Only the remote player's port is touched; port 1 is the host's.
find_pcsx2_config() {
  if [[ -n "${COUCHLINK_PCSX2_CONFIG:-}" ]]; then
    echo "${COUCHLINK_PCSX2_CONFIG}"
    return
  fi
  local win_user candidates=()
  win_user="$(powershell.exe -NoProfile -Command '$env:USERNAME' 2>/dev/null | tr -d '\r' || true)"
  if [[ -n "$win_user" ]]; then
    candidates+=(
      "/mnt/c/Users/$win_user/Documents/PCSX2/inis/PCSX2.ini"
      "/mnt/c/Program Files/PCSX2/inis/PCSX2.ini"
    )
  fi
  candidates+=("$HOME/.config/PCSX2/inis/PCSX2.ini")
  local c
  for c in "${candidates[@]}"; do
    [[ -f "$c" ]] && { echo "$c"; return; }
  done
}

link_pcsx2() {
  local cfg
  cfg="$(find_pcsx2_config)"
  if [[ -z "$cfg" ]]; then
    # Not an error: PCSX2 writes its ini on first launch.
    echo "==> PCSX2 config not found — skipping (launch PCSX2 once, or set COUCHLINK_PCSX2_CONFIG)"
    return 0
  fi

  # Only the xbox360 backend is wired here; ds4/winuhid enumerate through SDL
  # with an index we cannot predict without reading PCSX2's own device list.
  if [[ "${COUCHLINK_DS_VHID_BACKEND:-xbox360}" != "xbox360" ]]; then
    echo "==> PCSX2: backend ${COUCHLINK_DS_VHID_BACKEND:-} is not xbox360 — leaving Pad${PLAYER} alone"
    return 0
  fi

  # The companion now plugs in one ViGEm target per connected player, in join
  # order, so it fills XInput-0, XInput-1, XInput-2… as remote slots 1, 2, 3
  # connect. PLAYER is the emulator port (P2, P3, P4…), so PLAYER-2 is that
  # slot's XInput index — every player gets its own device, not slot 2's.
  local dev="${COUCHLINK_PCSX2_DEVICE:-XInput-$((PLAYER - 2))}"
  local section="Pad${PLAYER}"

  # Bindings read "Up = XInput-0/DPadUp", so match the value side. Matching the
  # start of the line silently never fired and rewrote the block every run.
  if awk -v sect="[$section]" -v dev="$dev" '
      { line = $0; sub(/\r$/, "", line) }
      line == sect { inblock = 1; next }
      inblock && line ~ /^\[/ { inblock = 0 }
      inblock && index(line, "= " dev "/") { found = 1 }
      END { exit found ? 0 : 1 }
    ' "$cfg" 2>/dev/null; then
    echo "==> PCSX2 ${section} already bound to ${dev}"
    return 0
  fi

  [[ -f "$cfg.couchlink.bak" ]] || cp -f "$cfg" "$cfg.couchlink.bak"

  local tmp
  tmp="$(mktemp)"
  # Drop the old [PadN] block, keep everything else, then append a fresh one.
  awk -v sect="[$section]" '
    { line = $0; sub(/\r$/, "", line) }
    line == sect { skip = 1; next }
    skip && line ~ /^\[/ { skip = 0 }
    !skip { print }
  ' "$cfg" > "$tmp"

  {
    echo
    echo "[$section]"
    echo "Type = DualShock2"
    echo "InvertL = 0"
    echo "InvertR = 0"
    echo "Deadzone = 0.00"
    echo "AxisScale = 1.33"
    echo "Up = ${dev}/DPadUp"
    echo "Right = ${dev}/DPadRight"
    echo "Down = ${dev}/DPadDown"
    echo "Left = ${dev}/DPadLeft"
    echo "Triangle = ${dev}/Y"
    echo "Circle = ${dev}/B"
    echo "Cross = ${dev}/A"
    echo "Square = ${dev}/X"
    echo "Select = ${dev}/Back"
    echo "Start = ${dev}/Start"
    echo "L1 = ${dev}/LeftShoulder"
    echo "R1 = ${dev}/RightShoulder"
    echo "L2 = ${dev}/+LeftTrigger"
    echo "R2 = ${dev}/+RightTrigger"
    echo "L3 = ${dev}/LeftStick"
    echo "R3 = ${dev}/RightStick"
    echo "LUp = ${dev}/-LeftY"
    echo "LRight = ${dev}/+LeftX"
    echo "LDown = ${dev}/+LeftY"
    echo "LLeft = ${dev}/-LeftX"
    echo "RUp = ${dev}/-RightY"
    echo "RRight = ${dev}/+RightX"
    echo "RDown = ${dev}/+RightY"
    echo "RLeft = ${dev}/-RightX"
  } >> "$tmp"

  # PCSX2 only enumerates XInput devices when that source is enabled.
  if grep -q '^\[InputSources\]' "$tmp"; then
    if grep -qE '^XInput *=' "$tmp"; then
      sed -i 's/^XInput *=.*/XInput = true/' "$tmp"
    else
      sed -i '/^\[InputSources\]/a XInput = true' "$tmp"
    fi
  else
    printf '\n[InputSources]\nXInput = true\n' >> "$tmp"
  fi

  if [[ ! -s "$tmp" ]] || ! grep -q "^\[$section\]" "$tmp"; then
    rm -f "$tmp"
    echo "==> refusing to write malformed PCSX2 config — left unchanged" >&2
    return 1
  fi

  cat "$tmp" > "$cfg"
  rm -f "$tmp"
  echo "==> PCSX2 ${section} bound to ${dev} (backup: $cfg.couchlink.bak)"
}

link_pcsx2 || true
