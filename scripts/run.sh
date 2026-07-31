#!/usr/bin/env bash
# One command to run couchlink: ./scripts/run.sh [host|client] [--local|--online]
# Auto-detects platform (Linux / WSL / macOS), starts signaling + TURN + host
# (or just the client) as background child processes of this one script, and
# tears them all down together on Ctrl-C. No separate terminals needed.
#
# Reachability:
#   host  --local   (default) same Wi‑Fi / LAN — join URL uses LAN IP, no UPnP/TURN
#   host  --online  internet — public IP + TURN + UPnP so a friend can open the URL anywhere
#   client --online requires host TURN (join URL or COUCHLINK_TURN_*); WSL auto ICE IPs
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-upnp.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-online-tunnel.sh"

usage() {
  cat <<EOF
usage: $0 [host|client] [--local|--online]

  host    start signaling + (optional TURN) + couchlink-host
  client  start couchlink-client (friend/player)

  --local   LAN only (default). Host: LAN join URL. Client: TURN optional.
  --online  Internet. Host: public IP + TURN + UPnP; on WSL also firewall,
            WSL portproxy, IPv6 invite / bore tunnel if the router blocks UPnP.
            Client: prompts for the host join URL if unset (TURN for NAT/WSL).

Platform is auto-detected (linux / wsl / macos).
EOF
}

ROLE="host"
MODE="local"
for arg in "$@"; do
  case "$arg" in
    host|client) ROLE="$arg" ;;
    --local) MODE="local" ;;
    --online) MODE="online" ;;
    -h|--help) usage; exit 0 ;;
    *)
      echo "unknown argument: $arg" >&2
      usage >&2
      exit 1
      ;;
  esac
done

PLATFORM="$(couchlink_detect_platform)"
echo "==> platform: $PLATFORM · role: $ROLE · mode: $MODE"

# Put Homebrew / cargo on PATH for macOS (system bash often lacks them).
export PATH="$(couchlink_tool_path "${HOME:-}")${PATH:+:$PATH}"

if [[ "$ROLE" == "host" && "$PLATFORM" == "macos" ]]; then
  echo "note: macOS host is video-only — no virtual DualSense (uinput is Linux/WSL)."
  echo "      Friend pad input will not inject; use Linux/WSL host for full co-play."
fi

[[ -f .env.couchlink ]] || cp .env.example .env.couchlink
# shellcheck disable=SC1091
source .env.couchlink

if [[ "$ROLE" == "host" && ( -z "${COUCHLINK_SESSION_ID:-}" || -z "${COUCHLINK_PIN:-}" ) ]]; then
  echo "==> no session set — generating one"
  eval "$(./scripts/gen_session.sh)"
  {
    echo "COUCHLINK_SESSION_ID=$COUCHLINK_SESSION_ID"
    echo "COUCHLINK_PIN=$COUCHLINK_PIN"
  } >> .env.couchlink
fi

# Reachability overrides — must win over whatever is in .env.couchlink for this run.
export COUCHLINK_MODE="$MODE"
PORT="${COUCHLINK_BIND##*:}"
PORT="${PORT:-8443}"

# On --online (WSL/Windows): Private profile + discovery + firewall + WSL
# portproxy + NATUPnP maps. Uses saved task CouchlinkElevatedUpnp after first UAC.
couchlink_try_upnp_online() {
  [[ "${COUCHLINK_SKIP_UPNP_PREP:-}" == "1" ]] && return 0
  local ok=0
  if [[ "$PLATFORM" == "wsl" || "$PLATFORM" == "windows" ]] && command -v powershell.exe >/dev/null 2>&1; then
    echo "==> --online: Windows prep (firewall + WSL portproxy + UPnP)"
    set +e
    bash "$ROOT/scripts/enable-upnp.sh"
    local ec=$?
    set -e
    [[ "$ec" -eq 0 ]] && ok=1
    # Retry map-only COM helper if prep left IGD visible but map exit was 2.
    if [[ "$ok" != "1" ]]; then
      local bridge_w
      bridge_w="$(wslpath -w "$ROOT/scripts/windows/open-ports-upnp.ps1" 2>/dev/null || true)"
      if [[ -n "${bridge_w:-}" ]]; then
        powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$bridge_w" && ok=1 || true
      fi
    fi
  fi
  if upnp_open "$PORT" tcp "signaling"; then
    ok=1
  fi
  upnp_open 3478 udp "turn" || true
  upnp_open 3478 tcp "turn" || true
  return $((1 - ok))
}

# When the router won't forward IPv4:
#   1) cloudflared HTTPS signaling (browser WebCodecs needs a secure context)
#   2) IPv6 TURN/invite when Windows has a global IPv6 (UDP/TCP without router NAT)
#   3) bore signaling-only last resort — never put TURN on bore
couchlink_apply_online_fallback() {
  local public_ip="$1"
  local v6=""
  v6="$(couchlink_read_public_ipv6 2>/dev/null || true)"

  local used_cf=0
  if couchlink_start_cloudflared "$ROOT" "$PORT"; then
    used_cf=1
    # Host still dials loopback; friends get https://*.trycloudflare.com
    export COUCHLINK_INVITE_SIGNALING="${COUCHLINK_CF_URL/https:/wss:}/ws"
    echo "==> HTTPS invite via cloudflared (WebCodecs unlocked in browser)"
  fi

  if [[ -n "$v6" ]]; then
    local v6br
    v6br="$(couchlink_bracket_host "$v6")"
    export COUCHLINK_TURN_URL="turn:${v6br}:3478"
    export COUCHLINK_TURN_EXTERNAL_IP="$v6"
    if [[ "$used_cf" != "1" ]]; then
      export COUCHLINK_INVITE_SIGNALING="ws://${v6br}:${PORT}/ws"
    fi
    echo "==> TURN on public IPv6 ${v6} (no IPv4 port forward needed)"
    return 0
  fi

  # No IPv6 — keep TURN on WAN IPv4 (needs UPnP/forward to actually work).
  export COUCHLINK_TURN_URL="turn:${public_ip}:3478"
  export COUCHLINK_TURN_EXTERNAL_IP="$public_ip"

  if [[ "$used_cf" == "1" ]]; then
    echo "==> WARN: no public IPv6 — TURN still ${public_ip}:3478 (forward UDP/TCP 3478 if ICE fails)"
    return 0
  fi

  if couchlink_start_bore_signaling "$ROOT" "$PORT"; then
    export COUCHLINK_INVITE_SIGNALING="ws://bore.pub:${COUCHLINK_BORE_SIG_PORT}/ws"
    echo "==> bore signaling only; TURN remains ${public_ip}:3478"
    echo "    browser WebCodecs needs https — prefer cloudflared; native client is fine on http"
    return 0
  fi

  echo "==> UPnP incomplete — keeping IPv4 invite ${public_ip} (may need manual forward)"
  echo "    forward TCP ${PORT} + UDP/TCP 3478 to this PC, or re-run ./scripts/enable-upnp.sh"
  return 1
}

if [[ "$ROLE" == "host" ]]; then
  if [[ "$MODE" == "local" ]]; then
    LAN_IP="$(upnp_local_ip)"
    LAN_IP="${LAN_IP:-127.0.0.1}"
    export COUCHLINK_SIGNALING="ws://${LAN_IP}:${PORT}/ws"
    # Don't advertise a public TURN relay on a LAN session.
    unset COUCHLINK_TURN_URL || true
    echo "==> local mode — join URL will use LAN IP ${LAN_IP} (no UPnP / TURN)"
  else
    PUBLIC_IP="${COUCHLINK_PUBLIC_IP:-}"
    if [[ -z "$PUBLIC_IP" ]]; then
      PUBLIC_IP="$(curl -fsS --max-time 5 ifconfig.me 2>/dev/null || true)"
    fi
    if [[ -z "$PUBLIC_IP" ]]; then
      echo "online mode needs a public IP (curl ifconfig.me failed)." >&2
      echo "Set COUCHLINK_PUBLIC_IP in .env.couchlink and re-run." >&2
      exit 1
    fi
    export COUCHLINK_PUBLIC_IP="$PUBLIC_IP"
    # Host must dial signaling on loopback/LAN — WSL/NAT often cannot hairpin
    # back to the public IP. Friends still get the public invite URL below.
    export COUCHLINK_SIGNALING="ws://127.0.0.1:${PORT}/ws"
    export COUCHLINK_INVITE_SIGNALING="ws://${PUBLIC_IP}:${PORT}/ws"
    export COUCHLINK_TURN_URL="turn:${PUBLIC_IP}:3478"
    echo "==> online mode — public IP ${PUBLIC_IP} (TURN + UPnP; host dials 127.0.0.1)"
    if couchlink_try_upnp_online; then
      echo "==> UPnP OK — ports should be reachable at ${PUBLIC_IP}"
      export COUCHLINK_SKIP_UPNP=1
    else
      couchlink_apply_online_fallback "$PUBLIC_IP" || true
    fi
  fi
elif [[ "$ROLE" == "client" ]]; then
  # Client reachability: remote joins need the host's TURN (UDP+TCP expanded in-process).
  # WSL auto-discovers the Windows LAN IP for ICE host candidates inside couchlink-client.
  # If COUCHLINK_JOIN_URL is unset, couchlink-client prompts in the terminal (or a GUI dialog).
  if [[ "$MODE" == "online" ]]; then
    if [[ -n "${COUCHLINK_JOIN_URL:-}" ]]; then
      echo "==> online client — join URL set (TURN from invite)"
    elif [[ -n "${COUCHLINK_TURN_URL:-}" && -n "${COUCHLINK_TURN_USER:-}" && -n "${COUCHLINK_TURN_PASS:-}" ]]; then
      echo "==> online client — TURN credentials from env"
    else
      echo "==> online client — will prompt for the host join URL (needed for TURN/NAT)"
    fi
  else
    echo "==> local client — will prompt for join URL if credentials are missing"
  fi
fi

PIDS=()
cleanup() {
  echo "==> shutting down"
  for pid in "${PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  for pid in "${COUCHLINK_TUNNEL_PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  for pid in "${COUCHLINK_BORE_PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  if [[ "$PLATFORM" == "wsl" && "$ROLE" == "host" ]]; then
    # Host started win-capture via powershell; stop it with the session.
    case "${COUCHLINK_WINDOWS_CAPTURE:-auto}" in
      0|false|local|off) ;;
      *)
        if command -v taskkill.exe >/dev/null 2>&1; then
          taskkill.exe /IM couchlink-win-capture.exe /F >/dev/null 2>&1 || true
        fi
        ;;
    esac
  fi
  wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

if [[ "$ROLE" == "host" ]]; then
  ./scripts/start-signaling.sh &
  PIDS+=($!)
  sleep 1
  if [[ "$MODE" == "online" ]]; then
    ./scripts/start-turn.sh &
    PIDS+=($!)
    sleep 1
  fi
  ./scripts/start-host.sh &
  PIDS+=($!)
else
  ./scripts/start-client.sh &
  PIDS+=($!)
fi

# wait -n needs bash ≥ 4.3; macOS /bin/bash is still 3.2.
if [[ "${BASH_VERSINFO[0]}" -gt 4 ]] \
  || { [[ "${BASH_VERSINFO[0]}" -eq 4 ]] && [[ "${BASH_VERSINFO[1]}" -ge 3 ]]; }; then
  wait -n "${PIDS[@]}"
else
  while true; do
    for pid in "${PIDS[@]}"; do
      if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid"
        exit $?
      fi
    done
    sleep 0.5
  done
fi
