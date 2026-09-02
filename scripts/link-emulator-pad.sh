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
# first remote player. The companion now plugs in exactly one target per
# couchlink player *slot* — created once, the first time that slot ever
# connects, and reused (never re-created) on every reconnect after that, so
# a second/third player's controller can never get silently swapped for a
# different seated player's mid-session anymore.
#
# What is NOT guaranteed: which XInput index a slot lands on. That's driver
# assignment order, decided by which slot connects to the companion FIRST
# in its lifetime — normally slot order, but if players connect out of
# order on a freshly (re)started companion, slot 2 could grab XInput-0
# before slot 1 ever has. PLAYER-1 below is therefore a working assumption,
# not a guarantee; if a player's binding looks wrong at the start of a
# session, check the companion's own log for "slot N: plugged in a new
# virtual controller" lines to see the real connect order.
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
  # Don't assume Documents lives where it normally does — OneDrive (or a
  # portable/custom install dir) redirects it, and a hardcoded guess just
  # silently misses the config every time that's true. Search for the file
  # itself instead, under the user's actual home and common install roots.
  local win_user
  win_user="$(powershell.exe -NoProfile -Command '$env:USERNAME' 2>/dev/null | tr -d '\r' || true)"
  local hit
  if [[ -n "$win_user" ]] && [[ -d "/mnt/c/Users/$win_user" ]]; then
    hit="$(find "/mnt/c/Users/$win_user" -maxdepth 6 -ipath '*/rpcs3/config/input_configs/global/Default.yml' 2>/dev/null | head -1)"
    [[ -n "$hit" ]] && { echo "$hit"; return; }
  fi
  [[ -f "$HOME/.config/rpcs3/input_configs/global/Default.yml" ]] && {
    echo "$HOME/.config/rpcs3/input_configs/global/Default.yml"
    return
  }
}

link_rpcs3() {
local CONFIG current want tmp
CONFIG="$(find_rpcs3_config)"
RPCS3_CONFIG_PATH="$CONFIG"
if [[ -z "$CONFIG" ]]; then
  echo "==> RPCS3 pad config not found — skipping (set COUCHLINK_RPCS3_CONFIG)" >&2
  RPCS3_STATUS=skipped
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
  RPCS3_STATUS=already
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
  RPCS3_STATUS=failed
  return 1
fi

cat "$tmp" > "$CONFIG"
rm -f "$tmp"
echo "==> RPCS3 Player $PLAYER linked to $HANDLER / $DEVICE (was ${current%%|*})"
echo "    backup: $CONFIG.couchlink.bak"
RPCS3_STATUS=linked

}

RPCS3_STATUS=skipped
link_rpcs3 || true

# ---------------------------------------------------------------- PCSX2 -----
# PCSX2 binds per button rather than by device name, so the whole Pad block is
# rewritten. Only the remote player's port is touched; port 1 is the host's.
find_pcsx2_config() {
  if [[ -n "${COUCHLINK_PCSX2_CONFIG:-}" ]]; then
    echo "${COUCHLINK_PCSX2_CONFIG}"
    return
  fi
  # Same reasoning as RPCS3 above: don't assume Documents, search for the
  # file itself. A system can have more than one PCSX2.ini lying around
  # (an unused portable-install default alongside the real one) — when
  # several turn up, the one PCSX2 actually writes to is the newest, so
  # pick by mtime rather than by search order.
  local win_user
  win_user="$(powershell.exe -NoProfile -Command '$env:USERNAME' 2>/dev/null | tr -d '\r' || true)"
  local roots=()
  [[ -n "$win_user" ]] && [[ -d "/mnt/c/Users/$win_user" ]] && roots+=("/mnt/c/Users/$win_user")
  [[ -d "/mnt/c/Program Files/PCSX2" ]] && roots+=("/mnt/c/Program Files/PCSX2")
  [[ -d "/mnt/c/Program Files (x86)/PCSX2" ]] && roots+=("/mnt/c/Program Files (x86)/PCSX2")
  if [[ ${#roots[@]} -gt 0 ]]; then
    local hit
    hit="$(find "${roots[@]}" -maxdepth 8 -iname 'PCSX2.ini' -printf '%T@ %p\n' 2>/dev/null \
      | sort -rn | head -1 | cut -d' ' -f2-)"
    [[ -n "$hit" ]] && { echo "$hit"; return; }
  fi
  [[ -f "$HOME/.config/PCSX2/inis/PCSX2.ini" ]] && {
    echo "$HOME/.config/PCSX2/inis/PCSX2.ini"
    return
  }
}

# Human-readable PS2 port for a PCSX2 [PadN] section, so the RESULT line says
# "1B" rather than leaving everyone to remember that Pad3 is not port 3.
pcsx2_port_name() {
  case "$1" in
    Pad1) echo "1A" ;;
    Pad2) echo "2A" ;;
    Pad3) echo "1B" ;;
    Pad4) echo "1C" ;;
    Pad5) echo "1D" ;;
    Pad6) echo "2B" ;;
    Pad7) echo "2C" ;;
    Pad8) echo "2D" ;;
    *) echo "" ;;
  esac
}

link_pcsx2() {
  local cfg
  cfg="$(find_pcsx2_config)"
  PCSX2_CONFIG_PATH="$cfg"
  if [[ -z "$cfg" ]]; then
    # Not an error: PCSX2 writes its ini on first launch.
    echo "==> PCSX2 config not found — skipping (launch PCSX2 once, or set COUCHLINK_PCSX2_CONFIG)"
    PCSX2_STATUS=skipped
    return 0
  fi

  # Only the xbox360 backend is wired here; ds4/winuhid enumerate through SDL
  # with an index we cannot predict without reading PCSX2's own device list.
  if [[ "${COUCHLINK_DS_VHID_BACKEND:-xbox360}" != "xbox360" ]]; then
    echo "==> PCSX2: backend ${COUCHLINK_DS_VHID_BACKEND:-} is not xbox360 — leaving Pad${PLAYER} alone"
    PCSX2_STATUS=skipped
    return 0
  fi

  # The companion plugs in exactly one ViGEm target per couchlink player
  # slot (created once ever, reused on every reconnect — see the RPCS3
  # comment above for the full reasoning and its one caveat: this assumes
  # slots connected in order the first time each one ever did).
  # PLAYER is the emulator port (P2, P3, P4…), so PLAYER-2 is that slot's
  # assumed XInput index — every player gets its own device, not slot 2's.
  local dev="${COUCHLINK_PCSX2_DEVICE:-XInput-$((PLAYER - 2))}"

  # PCSX2's [PadN] sections are NOT numbered sequentially across the ports.
  # The actual layout is:
  #
  #   Pad1 = port 1A     Pad2 = port 2A
  #   Pad3 = port 1B     Pad6 = port 2B
  #   Pad4 = port 1C     Pad7 = port 2C
  #   Pad5 = port 1D     Pad8 = port 2D
  #
  # i.e. Pad1/Pad2 are the two *base* ports, and Pad3-Pad5 are the extra
  # multitap slots hanging off port 1 (Pad6-Pad8 off port 2).
  #
  # This matters because a 4-player PS2 game reads its four players from ONE
  # multitap: ports 1A/1B/1C/1D. Binding "emulator player 2" to Pad2 puts that
  # player on port 2A — a port such a game never looks at — so their pad is
  # live in PCSX2 (it shows in the pad overlay, it registers in the settings)
  # and completely invisible to the game.
  #
  # Live-reproduced 2026-08-22 in Marvel Ultimate Alliance with 3 remote
  # players bound to Pad2/Pad3/Pad4: the game offered exactly two join slots
  # (the Pad3=1B and Pad4=1C players) and never saw the Pad2=2A player, who
  # could not register no matter what they pressed. Under the old assumption
  # that PadN numbering was sequential, all three should have shown up.
  #
  # So remote players go on the port-1 multitap chain: emulator player 2 ->
  # Pad3 (1B), player 3 -> Pad4 (1C), player 4 -> Pad5 (1D), leaving the host
  # on Pad1 (1A). Set COUCHLINK_PCSX2_MULTITAP=0 to fall back to the old
  # base-port layout (only ever correct for a single remote player, on 2A).
  local section
  if [[ "${COUCHLINK_PCSX2_MULTITAP:-1}" == "0" ]]; then
    section="Pad${PLAYER}"
  else
    section="Pad$((PLAYER + 1))"
  fi
  PCSX2_SECTION="$section"

  # Bindings read "Up = XInput-0/DPadUp", so match the value side. Matching the
  # start of the line silently never fired and rewrote the block every run.
  #
  # Also require Type != None: PCSX2's own in-game "toggle controller active"
  # UI (Marvel Ultimate Alliance and others) persists a disabled slot back to
  # this ini as "Type = None", *underneath* otherwise-correct button
  # bindings. The old check only looked at the bindings, so once a slot got
  # toggled off in-game it reported "already bound" forever and never
  # repaired it — the pad looked perfectly configured on disk while being
  # completely invisible to the running game. Live-reproduced 2026-08-22.
  if awk -v sect="[$section]" -v dev="$dev" '
      { line = $0; sub(/\r$/, "", line) }
      line == sect { inblock = 1; next }
      inblock && line ~ /^\[/ { inblock = 0 }
      inblock && line ~ /^Type = / { type = line; sub(/^Type = /, "", type) }
      inblock && index(line, "= " dev "/") { found = 1 }
      END { exit (found && type != "None") ? 0 : 1 }
    ' "$cfg" 2>/dev/null; then
    echo "==> PCSX2 ${section} already bound to ${dev}"
    PCSX2_STATUS=already
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

  # Ports 1B/1C/1D only exist when port 1 is actually a multitap — without
  # this the sections above are written but the game still sees a single
  # controller on port 1, which is the same invisible-player failure from a
  # different direction.
  if [[ "${COUCHLINK_PCSX2_MULTITAP:-1}" != "0" ]]; then
    if grep -q '^\[Pad\]' "$tmp"; then
      if grep -qE '^MultitapPort1 *=' "$tmp"; then
        sed -i 's/^MultitapPort1 *=.*/MultitapPort1 = true/' "$tmp"
      else
        sed -i '/^\[Pad\]/a MultitapPort1 = true' "$tmp"
      fi
    else
      printf '\n[Pad]\nMultitapPort1 = true\n' >> "$tmp"
    fi

    # Retire the old base-port binding for this player. Only Pad2 (port 2A)
    # is ever touched, and only when it still points at one of the virtual
    # devices this script itself wrote — a real controller someone deliberately
    # put on port 2A is left alone. Without this the pre-fix binding lingers,
    # so the same virtual pad answers on both 2A and 1B and a game that reads
    # both sees one player twice.
    # CRLF-aware throughout: PCSX2 writes this file with Windows line endings,
    # so matching "[Pad2]" without stripping the trailing \r silently never
    # fires (and rewriting without putting it back would corrupt the file).
    if awk -v want="$dev" '
        { line = $0; sub(/\r$/, "", line) }
        line == "[Pad2]" { inblock = 1; next }
        inblock && line ~ /^\[/ { inblock = 0 }
        inblock && index(line, "= " want "/") { found = 1 }
        END { exit found ? 0 : 1 }
      ' "$tmp" 2>/dev/null; then
      echo "==> PCSX2 retiring stale Pad2 (port 2A) binding for ${dev} — it now lives on ${section}"
      awk '
        { line = $0; cr = ""; if (sub(/\r$/, "", line)) cr = "\r" }
        line == "[Pad2]" { print line cr; inblock = 1; next }
        inblock && line ~ /^\[/ { inblock = 0 }
        inblock && line ~ /^Type *=/ { print "Type = None" cr; next }
        { print line cr }
      ' "$tmp" > "$tmp.pad2" && mv "$tmp.pad2" "$tmp"
    fi
  fi

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
    PCSX2_STATUS=failed
    return 1
  fi

  cat "$tmp" > "$cfg"
  rm -f "$tmp"
  echo "==> PCSX2 ${section} bound to ${dev} (backup: $cfg.couchlink.bak)"
  PCSX2_STATUS=linked
}

PCSX2_STATUS=skipped
PCSX2_CONFIG_PATH=""
PCSX2_SECTION=""
PCSX2_LIVE_APPLY=skipped

link_pcsx2 || true

# ---------------------------------------------------------------------------
# Piece B — runtime PCSX2 reconfiguration (in-memory Pad* apply)
#
# Piece A (ds-vhid preallocate) keeps ViGEm/XInput seats alive. That does NOT
# update PCSX2's already-loaded SettingsInterface. Upstream exposes no PINE/
# IPC opcode for this; the only supported entry that mutates live pads is the
# Controllers "Apply Profile" slot:
#
#   onApplyProfileClicked
#     -> Pad::CopyConfiguration(base, profile_ini, ...)
#     -> Host::CommitBaseSettingChanges()
#     -> g_emu_thread->applySettings()
#     -> VMManager::ApplySettings() / ReloadInputBindings
#
# We cannot call those C++ symbols from Couchlink. We *can* Invoke the same
# Qt button (UIA InvokePattern) so that exact handler runs. Disk PCSX2.ini is
# persistence only; the live path loads inputprofiles/couchlink.ini via Apply.
#
# Default: auto when pcsx2-qt is running. Opt out: COUCHLINK_PCSX2_LIVE_APPLY=0
# ---------------------------------------------------------------------------
write_pcsx2_input_profile() {
  local cfg="$1"
  local dir profile
  dir="$(dirname "$cfg")/inputprofiles"
  mkdir -p "$dir"
  profile="$dir/couchlink.ini"
  # Input profiles are Pad-centric; include Multitap + XInput so Apply
  # Profile enables the same sources the disk ini has.
  awk '
    { line = $0; sub(/\r$/, "", line) }
    line ~ /^\[/ {
      keep = (line == "[Pad]" || line == "[InputSources]" || line ~ /^\[Pad[1-8]\]$/)
    }
    keep { print }
  ' "$cfg" > "$profile"
  # Ensure the profile is non-empty and has at least one remote pad.
  if [[ ! -s "$profile" ]] || ! grep -qE '^\[Pad[3-5]\]' "$profile"; then
    echo "==> PCSX2 live-apply: refusing empty/incomplete couchlink input profile" >&2
    return 1
  fi
  echo "==> PCSX2 input profile synced: $profile" >&2
  printf '%s\n' "$profile"
}


# Pin EmuCore/InputProfileName=couchlink on the newest game settings ini so
# PCSX2's UpdateGameSettingsLayer Load()s inputprofiles/couchlink.ini as the
# pad overlay (headless Piece B — no Apply Profile UI). Best-effort: skip if
# no gamesettings yet (game never opened Properties).
pin_pcsx2_input_profile_name() {
  local cfg="$1"
  local gdir newest
  gdir="$(dirname "$cfg")/../gamessettings"
  # Portable layout: inis/../gamesettings OR Documents/PCSX2/gamesettings
  [[ -d "$gdir" ]] || gdir="$(dirname "$cfg")/gamesettings"
  [[ -d "$gdir" ]] || gdir="$(dirname "$(dirname "$cfg")")/gamesettings"
  [[ -d "$gdir" ]] || return 0
  newest="$(find "$gdir" -maxdepth 1 -type f -name '*.ini' -printf '%T@ %p\n' 2>/dev/null \
    | sort -rn | head -1 | cut -d' ' -f2-)"
  [[ -n "$newest" && -f "$newest" ]] || return 0
  if grep -qE '^InputProfileName *= *couchlink' "$newest" 2>/dev/null; then
    echo "==> PCSX2 game settings already pin InputProfileName=couchlink ($(basename "$newest"))" >&2
    return 0
  fi
  if grep -qE '^\[EmuCore\]' "$newest"; then
    if grep -qE '^InputProfileName *=' "$newest"; then
      sed -i 's/^InputProfileName *=.*/InputProfileName = couchlink/' "$newest"
    else
      sed -i '/^\[EmuCore\]/a InputProfileName = couchlink' "$newest"
    fi
  else
    printf '\n[EmuCore]\nInputProfileName = couchlink\n' >> "$newest"
  fi
  echo "==> PCSX2 pinned InputProfileName=couchlink on $(basename "$newest") (reload game settings / soft restart to Load)" >&2
}

pcsx2_live_apply_should_run() {
  # UIA Apply Profile is abandoned as primary (locksmith: use InputProfileName
  # layer + optional headless reload). Opt-in only: COUCHLINK_PCSX2_LIVE_APPLY=1
  case "${COUCHLINK_PCSX2_LIVE_APPLY:-0}" in
    1|true|on|yes) return 0 ;;
    *) return 1 ;;
  esac
}

pcsx2_live_apply_debounced() {
  local profile="$1"
  local stamp cooldown prev hash age now
  stamp="${XDG_RUNTIME_DIR:-/tmp}/couchlink-pcsx2-live-apply.stamp"
  cooldown="${COUCHLINK_PCSX2_LIVE_APPLY_COOLDOWN_SEC:-30}"
  hash="$(cksum "$profile" | awk '{print $1 "-" $2}')"
  now="$(date +%s)"
  if [[ -f "$stamp" ]]; then
    prev="$(awk 'NR==1 {print; exit}' "$stamp")"
    age=$(( now - $(stat -c %Y "$stamp" 2>/dev/null || echo 0) ))
    if [[ "$prev" == "$hash" && "$age" -ge 0 && "$age" -lt "$cooldown" ]]; then
      echo "==> PCSX2 live-apply debounced (${age}s < ${cooldown}s, profile unchanged)"
      return 1
    fi
  fi
  printf '%s\n' "$hash" > "$stamp"
  return 0
}

pcsx2_live_apply_if_running() {
  local cfg="$1"
  local profile
  PCSX2_LIVE_APPLY=skipped
  [[ -n "$cfg" && -f "$cfg" ]] || return 0
  case "$PCSX2_STATUS" in
    linked|already) ;;
    *) return 0 ;;
  esac

  # Always sync the input profile so Controllers -> Apply Profile (manual or
  # automated) has the current Pad3/4/5 map ready.
  profile="$(write_pcsx2_input_profile "$cfg")" || {
    PCSX2_LIVE_APPLY=failed
    return 0
  }
  pin_pcsx2_input_profile_name "$cfg" || true

  # Honor hard-off before any Windows process probe (avoids a ~multi-second
  # powershell round-trip when the operator disabled live-apply).
  if ! pcsx2_live_apply_should_run; then
    echo "==> PCSX2 live-apply disabled (COUCHLINK_PCSX2_LIVE_APPLY=0)"
    PCSX2_LIVE_APPLY=disabled
    return 0
  fi

  if ! powershell.exe -NoProfile -Command \
      'if (Get-Process pcsx2-qt -ErrorAction SilentlyContinue) { exit 0 } else { exit 1 }' \
      >/dev/null 2>&1; then
    echo "==> PCSX2 not running — disk + input profile only (Load() on next launch)"
    PCSX2_LIVE_APPLY=skipped
    return 0
  fi

  if ! pcsx2_live_apply_debounced "$profile"; then
    PCSX2_LIVE_APPLY=debounced
    return 0
  fi

  local ps1 win_ps1
  ps1="$ROOT/scripts/windows/pcsx2-live-apply-pads.ps1"
  if [[ ! -f "$ps1" ]]; then
    echo "==> PCSX2 live-apply script missing: $ps1" >&2
    PCSX2_LIVE_APPLY=failed
    return 0
  fi
  win_ps1="$(wslpath -w "$ps1")"
  echo "==> PCSX2 live-apply: Invoke Apply Profile 'couchlink' (Pad::CopyConfiguration path)"
  if powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$win_ps1" \
      -ProfileName couchlink 2>&1; then
    PCSX2_LIVE_APPLY=applied
  else
    echo "==> PCSX2 live-apply failed — Controllers -> Editing Profile: couchlink -> Apply Profile" >&2
    PCSX2_LIVE_APPLY=failed
  fi
}

if [[ -n "$PCSX2_CONFIG_PATH" ]]; then
  pcsx2_live_apply_if_running "$PCSX2_CONFIG_PATH" || true
fi

# A single machine-parseable summary line so the host can surface real
# per-player status (not just "linked"/"skipped") up to the debug UI instead
# of only ever knowing "the script exited 0". backend/device/handler describe
# what was actually configured; the two config paths (blank when not found)
# say exactly which file the binding did or didn't touch, since "skipped"
# alone gives the player nothing to act on when their controller doesn't work.
# JSON via jq rather than hand-quoted key=value text: device names and config
# paths can contain spaces (they already do — "XInput Pad #1", OneDrive
# paths), and jq handles that escaping correctly instead of a fragile ad hoc
# `key="$value"` format the Rust side would have to hand-parse.
echo "RESULT $(jq -nc \
  --arg player "$PLAYER" \
  --arg backend "${COUCHLINK_DS_VHID_BACKEND:-xbox360}" \
  --arg handler "$HANDLER" \
  --arg device "$DEVICE" \
  --arg rpcs3 "$RPCS3_STATUS" \
  --arg rpcs3_config "$RPCS3_CONFIG_PATH" \
  --arg pcsx2 "$PCSX2_STATUS" \
  --arg pcsx2_config "$PCSX2_CONFIG_PATH" \
  --arg pcsx2_section "$PCSX2_SECTION" \
  --arg pcsx2_port "$(pcsx2_port_name "$PCSX2_SECTION")" \
  --arg pcsx2_live_apply "$PCSX2_LIVE_APPLY" \
  '{player: ($player | tonumber), backend: $backend, handler: $handler, device: $device,
    rpcs3: $rpcs3, rpcs3_config: $rpcs3_config, pcsx2: $pcsx2, pcsx2_config: $pcsx2_config,
    pcsx2_section: $pcsx2_section, pcsx2_port: $pcsx2_port, pcsx2_live_apply: $pcsx2_live_apply}')"
