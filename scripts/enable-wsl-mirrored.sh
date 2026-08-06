#!/usr/bin/env bash
# Give WSL the Windows network identity, so it can actually serve TURN.
#
# The problem this solves is structural, not a misconfiguration:
#
#   * WSL2's default NAT mode gives the VM a private IPv4 and no IPv6 at all.
#   * The only inbound bridge is `netsh interface portproxy`, which is TCP-only.
#   * coturn needs inbound UDP, so no amount of portproxy can reach it.
#   * The invite therefore advertised the *Windows* IPv6 for TURN, an address
#     the relay could never answer on. Friends whose NAT refuses a direct path
#     gathered no `typ relay` candidate and ICE failed — silently, and in a way
#     that looks exactly like the friend's network being broken.
#
# Mirrored networking removes the NAT layer entirely: the WSL instance shares
# the host's interfaces and addresses, so coturn binds the same global IPv6 the
# invite advertises, and inbound UDP simply works with no router involvement.
#
# Requires Windows 11 22H2 (build 22621) or newer. Older builds have no
# mirrored mode, and the honest answer there is a router port forward.
set -euo pipefail

CONF_MODE="${1:-mirrored}"

if ! grep -qi microsoft /proc/version 2>/dev/null; then
  echo "==> not WSL — nothing to do (native Linux owns its own addresses)"
  exit 0
fi

if ! command -v powershell.exe >/dev/null 2>&1; then
  echo "error: powershell.exe not found — cannot reach the Windows side" >&2
  exit 1
fi

build="$(powershell.exe -NoProfile -Command \
  '[int](Get-CimInstance Win32_OperatingSystem).BuildNumber' 2>/dev/null | tr -d ' \r\n')"
if [[ ! "$build" =~ ^[0-9]+$ ]]; then
  echo "error: could not read the Windows build number" >&2
  exit 1
fi
if (( build < 22621 )); then
  echo "==> Windows build $build is older than 22621 — mirrored networking is unavailable."
  echo "    Relay will need a router forward instead: UDP+TCP 3478 to this PC."
  exit 2
fi

win_user="$(powershell.exe -NoProfile -Command '$env:USERNAME' 2>/dev/null | tr -d ' \r\n')"
if [[ -z "$win_user" ]]; then
  echo "error: could not determine the Windows username" >&2
  exit 1
fi
conf="/mnt/c/Users/${win_user}/.wslconfig"

if [[ -f "$conf" ]] && grep -qiE "^[[:space:]]*networkingMode[[:space:]]*=[[:space:]]*${CONF_MODE}" "$conf"; then
  echo "==> .wslconfig already sets networkingMode=${CONF_MODE}"
  if ip -6 addr show scope global 2>/dev/null | grep -q inet6; then
    echo "==> WSL holds a global IPv6 — mirrored mode is live, TURN can serve on it"
    exit 0
  fi
  echo "==> but WSL still has no global IPv6 — run 'wsl --shutdown' from Windows and reopen"
  exit 3
fi

# Preserve anything already in .wslconfig; only touch the [wsl2] networkingMode.
[[ -f "$conf" ]] && cp -f "$conf" "$conf.couchlink.bak"
tmp="$(mktemp)"
if [[ -f "$conf" ]]; then
  # Drop any existing networkingMode line, keep the rest verbatim.
  grep -viE '^[[:space:]]*networkingMode[[:space:]]*=' "$conf" > "$tmp" || true
else
  : > "$tmp"
fi
if ! grep -qiE '^\[wsl2\]' "$tmp"; then
  printf '[wsl2]\n' >> "$tmp"
fi
# Insert directly under [wsl2] so it lands in the right section.
awk -v mode="$CONF_MODE" '
  BEGIN { done = 0 }
  { line = $0; sub(/\r$/, "", line); print line }
  !done && tolower(line) ~ /^\[wsl2\]/ { print "networkingMode=" mode; done = 1 }
' "$tmp" > "$tmp.out"

if ! grep -qiE "^networkingMode=${CONF_MODE}" "$tmp.out"; then
  rm -f "$tmp" "$tmp.out"
  echo "==> refusing to write a .wslconfig without the setting — left unchanged" >&2
  exit 1
fi

cat "$tmp.out" > "$conf"
rm -f "$tmp" "$tmp.out"

echo "==> wrote networkingMode=${CONF_MODE} to $conf"
[[ -f "$conf.couchlink.bak" ]] && echo "    backup: $conf.couchlink.bak"
echo
echo "    Now run this from a Windows terminal, then reopen WSL:"
echo "        wsl --shutdown"
echo
echo "    After that, WSL shares the Windows addresses: coturn binds the same"
echo "    global IPv6 the invite advertises, and no router forward is needed."
