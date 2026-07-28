#!/usr/bin/env bash
# One command to run couchlink: ./scripts/run.sh [host|client]
# Detects platform (WSL / Linux native / macOS), starts signaling + TURN + host
# (or just the client) as background child processes of this one script, and
# tears them all down together on Ctrl-C. No separate terminals needed.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ROLE="${1:-host}"
case "$ROLE" in
  host|client) ;;
  *) echo "usage: $0 [host|client]"; exit 1 ;;
esac

PLATFORM="linux"
if grep -qi microsoft /proc/version 2>/dev/null; then
  PLATFORM="wsl"
elif [[ "$(uname -s)" == "Darwin" ]]; then
  PLATFORM="macos"
fi
echo "==> platform: $PLATFORM · role: $ROLE"

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

PIDS=()
cleanup() {
  echo "==> shutting down"
  for pid in "${PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

if [[ "$ROLE" == "host" ]]; then
  ./scripts/start-signaling.sh &
  PIDS+=($!)
  sleep 1
  ./scripts/start-turn.sh &
  PIDS+=($!)
  sleep 1
  ./scripts/start-host.sh &
  PIDS+=($!)
else
  ./scripts/start-client.sh &
  PIDS+=($!)
fi

wait -n "${PIDS[@]}"
