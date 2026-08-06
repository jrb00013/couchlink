#!/usr/bin/env bash
# Undo the direct-connection setup: WireGuard tunnel + WSL mirrored networking.
#
# The two are separate changes that were made together, so they are reverted
# together here. Either can be skipped.
#
#   ./scripts/revert-wireguard-network.sh              revert both
#   ./scripts/revert-wireguard-network.sh --check      report state, change nothing
#   ./scripts/revert-wireguard-network.sh --wg-only    tunnel only, keep mirrored
#   ./scripts/revert-wireguard-network.sh --net-only   networking only, keep tunnel
#
# What each revert costs you:
#
#   WireGuard off   -> friends fall back to the Cloudflare quick tunnel, with a
#                      fresh random URL every restart.
#   Mirrored off    -> WSL returns to its own private IP with no IPv6, so the
#                      TURN relay stops being reachable and friends behind a
#                      strict NAT fail ICE. netsh portproxy (TCP-only) comes
#                      back as the inbound bridge, recreated on the next
#                      `run.sh host --online`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

DO_WG=1
DO_NET=1
CHECK_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --check) CHECK_ONLY=1 ;;
    --wg-only) DO_NET=0 ;;
    --net-only) DO_WG=0 ;;
    -h|--help) sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $arg (try --help)" >&2; exit 1 ;;
  esac
done

have_ps() { command -v powershell.exe >/dev/null 2>&1; }
is_wsl() { grep -qi microsoft /proc/version 2>/dev/null; }

# ---------------------------------------------------------------- report -----
wg_state="down"
if ip link show wg0 >/dev/null 2>&1; then
  wg_state="up (WSL wg0)"
elif have_ps && powershell.exe -NoProfile -Command \
    "if (Get-Service -Name 'WireGuardTunnel\$couchlink' -ErrorAction SilentlyContinue) { 'yes' }" \
    2>/dev/null | tr -d ' \r\n' | grep -q yes; then
  wg_state="up (Windows service)"
fi

net_state="nat"
if ip -6 addr show scope global 2>/dev/null | grep -q 'inet6 [23]'; then
  net_state="mirrored"
fi

echo "==> WireGuard:  $wg_state"
echo "==> networking: $net_state"
[[ "$CHECK_ONLY" == "1" ]] && exit 0

# ------------------------------------------------------------- wireguard -----
if [[ "$DO_WG" == "1" ]]; then
  # The tunnel can live on either side: wg-quick inside WSL, or the Windows
  # WireGuard service installed by enable-wireguard.ps1. Take down whichever
  # exists rather than assuming.
  if ip link show wg0 >/dev/null 2>&1; then
    echo "==> taking down WSL wg0"
    if [[ -f "$ROOT/infra/wireguard/wg0-host.conf" ]]; then
      sudo wg-quick down "$ROOT/infra/wireguard/wg0-host.conf" || \
        sudo ip link delete wg0 || true
    else
      sudo ip link delete wg0 || true
    fi
  fi

  if have_ps; then
    echo "==> removing the Windows WireGuard tunnel (approve the UAC prompt)"
    powershell.exe -NoProfile -Command "Start-Process powershell -Verb RunAs -Wait -ArgumentList '-NoProfile','-Command','if (Get-Service -Name ''WireGuardTunnel\$couchlink'' -ErrorAction SilentlyContinue) { & \"\$env:ProgramFiles\\WireGuard\\wireguard.exe\" /uninstalltunnelservice couchlink }'" \
      >/dev/null 2>&1 || echo "    (could not remove it automatically — see manual step below)"
  fi
fi

# ------------------------------------------------------------ networking -----
if [[ "$DO_NET" == "1" ]]; then
  if ! is_wsl; then
    echo "==> not WSL — no networking mode to revert"
  elif ! have_ps; then
    echo "error: powershell.exe not found — cannot reach the Windows side" >&2
    exit 1
  else
    win_user="$(powershell.exe -NoProfile -Command '$env:USERNAME' 2>/dev/null | tr -d ' \r\n')"
    if [[ -z "$win_user" ]]; then
      echo "error: could not read the Windows username" >&2
      exit 1
    fi
    CONF="/mnt/c/Users/${win_user}/.wslconfig"

    if [[ ! -f "$CONF" ]]; then
      echo "==> no .wslconfig — already on the default (NAT)"
    else
      cp -f "$CONF" "$CONF.revert.bak"
      tmp="$(mktemp)"
      grep -viE '^[[:space:]]*networkingMode[[:space:]]*=' "$CONF" > "$tmp" || true
      grep -qiE '^\[wsl2\]' "$tmp" || printf '[wsl2]\n' >> "$tmp"
      awk '
        BEGIN { done = 0 }
        { line = $0; sub(/\r$/, "", line); print line }
        !done && tolower(line) ~ /^\[wsl2\]/ { print "networkingMode=nat"; done = 1 }
      ' "$tmp" > "$tmp.out"

      # Never leave a .wslconfig that lost the user's other settings.
      if ! grep -qiE '^networkingMode=nat$' "$tmp.out"; then
        rm -f "$tmp" "$tmp.out"
        echo "==> refusing to write a .wslconfig missing the setting — left unchanged" >&2
        exit 1
      fi
      cat "$tmp.out" > "$CONF"
      rm -f "$tmp" "$tmp.out"
      echo "==> set networkingMode=nat in $CONF (backup: $CONF.revert.bak)"
    fi
  fi
fi

echo
echo "Finish from a Windows terminal:"
[[ "$DO_NET" == "1" ]] && echo "    wsl --shutdown        # applies the networking change"
[[ "$DO_WG" == "1" ]] && cat <<'NOTE'
    # if the tunnel is still listed in the WireGuard UI, remove it there,
    # or run as administrator:
    #   & "$env:ProgramFiles\WireGuard\wireguard.exe" /uninstalltunnelservice couchlink
NOTE
echo
echo "Then ./scripts/run.sh host --online falls back to the Cloudflare tunnel."
