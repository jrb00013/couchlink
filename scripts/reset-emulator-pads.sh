#!/usr/bin/env bash
# Wipe the remote player controller slots in PCSX2 and RPCS3 back to empty.
#
# Usage:
#   ./scripts/reset-emulator-pads.sh              # both emulators, keep player 1
#   ./scripts/reset-emulator-pads.sh --all        # clear player 1 too (your own pad)
#   ./scripts/reset-emulator-pads.sh --pcsx2      # PCSX2 only
#   ./scripts/reset-emulator-pads.sh --rpcs3      # RPCS3 only
#   COUCHLINK_PCSX2_CONFIG=... COUCHLINK_RPCS3_CONFIG=... ./scripts/reset-emulator-pads.sh
#
# Why this exists: couchlink's pad linking is idempotent, so a *wrong* binding
# that already looks bound is never repaired — it just keeps reporting "already
# bound". Sessions accumulate cruft that way (a slot left on the wrong port by
# an older mapping, a Type=None a game's own controller-toggle UI wrote back, a
# device index from a companion that no longer exists). Clearing the slots
# gives couchlink a clean surface to rebind onto, which is otherwise a fiddly
# hand-edit of a long config with CRLF line endings.
#
# Player 1 is the host's own controller and is preserved by default — clearing
# it means re-binding your own pad by hand in the emulator's UI.
#
# The emulator MUST NOT be running: both read their config at startup and
# rewrite it from memory on exit, so edits made while one is open are silently
# discarded. The script refuses rather than write changes that would be lost.
set -euo pipefail

DO_PCSX2=1
DO_RPCS3=1
CLEAR_P1=0
for arg in "$@"; do
  case "$arg" in
    --all) CLEAR_P1=1 ;;
    --pcsx2) DO_RPCS3=0 ;;
    --rpcs3) DO_PCSX2=0 ;;
    -h|--help) sed -n '2,26p' "$0" | sed 's/^# \?//'; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 1 ;;
  esac
done

is_wsl() { grep -qi microsoft /proc/version 2>/dev/null; }

win_home() {
  local u
  if is_wsl && command -v powershell.exe >/dev/null 2>&1; then
    u="$(powershell.exe -NoProfile -Command '$env:USERNAME' 2>/dev/null | tr -d '\r' || true)"
    [[ -n "${u:-}" && -d "/mnt/c/Users/$u" ]] && echo "/mnt/c/Users/$u"
  fi
}

# Refuse while the emulator holds its config: it overwrites on exit.
running_guard() {
  local image="$1" pretty="$2"
  if command -v tasklist.exe >/dev/null 2>&1 \
    && tasklist.exe /FI "IMAGENAME eq $image" 2>/dev/null | grep -qi "${image%.exe}"; then
    echo "error: $pretty is running — close it first." >&2
    echo "  It only reads its config at startup and rewrites it on exit, so any" >&2
    echo "  change made now would be silently discarded." >&2
    return 1
  fi
  return 0
}

# ------------------------------------------------------------------ PCSX2 ---
find_pcsx2_config() {
  if [[ -n "${COUCHLINK_PCSX2_CONFIG:-}" ]]; then echo "${COUCHLINK_PCSX2_CONFIG}"; return; fi
  # Don't assume Documents (OneDrive redirects it); when several exist prefer
  # the newest, since that is the one PCSX2 actually writes to.
  local roots=() hit wh
  wh="$(win_home)"; [[ -n "$wh" ]] && roots+=("$wh")
  [[ -d "/mnt/c/Program Files/PCSX2" ]] && roots+=("/mnt/c/Program Files/PCSX2")
  if [[ ${#roots[@]} -gt 0 ]]; then
    hit="$(find "${roots[@]}" -maxdepth 8 -iname 'PCSX2.ini' -printf '%T@ %p\n' 2>/dev/null \
      | sort -rn | head -1 | cut -d' ' -f2-)"
    [[ -n "$hit" ]] && { echo "$hit"; return; }
  fi
  [[ -f "$HOME/.config/PCSX2/inis/PCSX2.ini" ]] && echo "$HOME/.config/PCSX2/inis/PCSX2.ini"
}

reset_pcsx2() {
  local cfg first_pad backup
  cfg="$(find_pcsx2_config)"
  if [[ -z "${cfg:-}" || ! -f "$cfg" ]]; then
    echo "==> PCSX2 config not found — skipped (set COUCHLINK_PCSX2_CONFIG)"
    return 0
  fi
  running_guard "pcsx2-qt.exe" "PCSX2" || return 1

  backup="$cfg.pad-reset.$(date +%s).bak"
  cp -f "$cfg" "$backup"
  first_pad=2
  [[ "$CLEAR_P1" == "1" ]] && first_pad=1

  python3 - "$cfg" "$first_pad" <<'PYEOF'
import re, sys
path, first_pad = sys.argv[1], int(sys.argv[2])
with open(path, "r", newline="") as f:
    lines = f.read().split("\n")

section_re = re.compile(r"^\[(Pad([1-8]))\]$")
out, current, cleared = [], None, []
for line in lines:
    # PCSX2 writes CRLF. Strip \r to match, keep the original ending on output
    # or the file ends up with mixed line endings.
    stripped = line.rstrip("\r")
    eol = line[len(stripped):]
    m = section_re.match(stripped)
    if m:
        current = m.group(1) if int(m.group(2)) >= first_pad else None
        if current:
            cleared.append(current)
        out.append(line); continue
    if stripped.startswith("[") and stripped.endswith("]"):
        current = None; out.append(line); continue
    if current:
        # Keep the block, empty it: Type=None and no bindings — exactly what
        # PCSX2 itself writes for an unused port.
        if re.match(r"^Type\s*=", stripped):
            out.append("Type = None" + eol)
        elif stripped.strip() == "":
            out.append(line)
        continue
    out.append(line)

with open(path, "w", newline="") as f:
    f.write("\n".join(out))
order = sorted(set(cleared), key=lambda s: int(s[3:]))
print("    cleared: " + (", ".join(order) if order else "(nothing)"))
PYEOF
  echo "==> PCSX2 pads reset in $cfg"
  echo "    backup: $backup"
}

# ------------------------------------------------------------------ RPCS3 ---
find_rpcs3_config() {
  if [[ -n "${COUCHLINK_RPCS3_CONFIG:-}" ]]; then echo "${COUCHLINK_RPCS3_CONFIG}"; return; fi
  local wh hit
  wh="$(win_home)"
  if [[ -n "$wh" ]]; then
    hit="$(find "$wh" -maxdepth 6 -ipath '*/rpcs3/config/input_configs/global/Default.yml' 2>/dev/null | head -1)"
    [[ -n "$hit" ]] && { echo "$hit"; return; }
  fi
  [[ -f "$HOME/.config/rpcs3/input_configs/global/Default.yml" ]] \
    && echo "$HOME/.config/rpcs3/input_configs/global/Default.yml"
}

reset_rpcs3() {
  local cfg first_player backup
  cfg="$(find_rpcs3_config)"
  if [[ -z "${cfg:-}" || ! -f "$cfg" ]]; then
    echo "==> RPCS3 config not found — skipped (set COUCHLINK_RPCS3_CONFIG)"
    return 0
  fi
  running_guard "rpcs3.exe" "RPCS3" || return 1

  backup="$cfg.pad-reset.$(date +%s).bak"
  cp -f "$cfg" "$backup"
  first_player=2
  [[ "$CLEAR_P1" == "1" ]] && first_player=1

  python3 - "$cfg" "$first_player" <<'PYEOF'
import re, sys
path, first_player = sys.argv[1], int(sys.argv[2])
with open(path, "r", newline="") as f:
    lines = f.read().split("\n")

player_re = re.compile(r"^Player (\d+) Input:$")
out, current, cleared = [], None, []
for line in lines:
    # RPCS3 writes CRLF on Windows — same handling as the PCSX2 block above.
    stripped = line.rstrip("\r")
    eol = line[len(stripped):]
    m = player_re.match(stripped)
    if m:
        n = int(m.group(1))
        current = n if n >= first_player else None
        if current:
            cleared.append(n)
        out.append(line); continue
    if current is not None:
        # Only Handler/Device are retargeted; the Config block underneath is
        # left alone so per-player tuning survives a reset. "Null" is the exact
        # handler/device RPCS3 itself writes for an unused player.
        if re.match(r"^  Handler:", stripped):
            out.append('  Handler: "Null"' + eol); continue
        if re.match(r"^  Device:", stripped):
            out.append('  Device: "Null"' + eol); continue
    out.append(line)

with open(path, "w", newline="") as f:
    f.write("\n".join(out))
order = sorted(set(cleared))
print("    cleared: " + (", ".join("Player %d" % n for n in order) if order else "(nothing)"))
PYEOF
  echo "==> RPCS3 pads reset in $cfg"
  echo "    backup: $backup"
}

rc=0
[[ "$DO_PCSX2" == "1" ]] && { reset_pcsx2 || rc=1; }
[[ "$DO_RPCS3" == "1" ]] && { reset_rpcs3 || rc=1; }

if [[ "$CLEAR_P1" == "0" ]]; then
  echo "    player 1 (your own controller) left untouched — use --all to clear it too"
fi
echo "    remote players rebind automatically when the host starts (emulator_pad::prebind_all)"
exit "$rc"
