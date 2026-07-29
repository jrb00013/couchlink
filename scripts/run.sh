#!/usr/bin/env bash
# One command to run couchlink: ./scripts/run.sh [host|client] [--local|--online]
# Detects platform (WSL / Linux native / macOS), starts signaling + TURN + host
# (or just the client) as background child processes of this one script, and
# tears them all down together on Ctrl-C. No separate terminals needed.
#
# Reachability (host only):
#   --local   (default) same Wi‑Fi / LAN — join URL uses your LAN IP, no UPnP/TURN
#   --online  internet  — public IP + TURN + UPnP so a friend can open the URL anywhere
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-upnp.sh"

usage() {
  cat <<EOF
usage: $0 [host|client] [--local|--online]

  host    start signaling + (optional TURN) + couchlink-host
  client  start couchlink-client (friend/player)

  --local   LAN only (default for host). Join URL uses your LAN IP.
  --online  Internet. Fetches public IP, starts TURN, opens ports via UPnP.
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

PLATFORM="linux"
if grep -qi microsoft /proc/version 2>/dev/null; then
  PLATFORM="wsl"
elif [[ "$(uname -s)" == "Darwin" ]]; then
  PLATFORM="macos"
fi
echo "==> platform: $PLATFORM · role: $ROLE · mode: $MODE"

if [[ "$ROLE" == "host" && "$PLATFORM" == "macos" ]]; then
  echo "macOS has no uinput — the host's virtual DualSense injection needs Linux or WSL."
  echo "Run the host role from a Linux machine or WSL; macOS can still run './scripts/run.sh client'."
  exit 1
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
    export COUCHLINK_SIGNALING="ws://${PUBLIC_IP}:${PORT}/ws"
    export COUCHLINK_TURN_URL="turn:${PUBLIC_IP}:3478"
    echo "==> online mode — public IP ${PUBLIC_IP} (TURN + UPnP)"
  fi
fi

PIDS=()
cleanup() {
  echo "==> shutting down"
  for pid in "${PIDS[@]:-}"; do
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

wait -n "${PIDS[@]}"
