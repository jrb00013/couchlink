#!/usr/bin/env bash
# Run a local coturn TURN relay so friends behind symmetric NAT / CGNAT can still
# join — STUN alone (webrtc_peer.rs) doesn't punch through those. Auto-generates
# credentials into .env.couchlink on first run.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="$ROOT/.env.couchlink"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-upnp.sh"

if ! command -v turnserver >/dev/null; then
  echo "coturn not installed — installing (needs sudo)"
  if command -v apt-get >/dev/null; then
    sudo apt-get update -qq && sudo apt-get install -y -qq coturn
  else
    echo "Install coturn manually: https://github.com/coturn/coturn"
    exit 1
  fi
fi

# shellcheck disable=SC1090
[[ -f "$ENV_FILE" ]] && source "$ENV_FILE"

if [[ -z "${COUCHLINK_TURN_USER:-}" || -z "${COUCHLINK_TURN_PASS:-}" ]]; then
  COUCHLINK_TURN_USER="cl$(head -c 4 /dev/urandom | xxd -p)"
  COUCHLINK_TURN_PASS="$(head -c 16 /dev/urandom | xxd -p)"
  {
    echo "COUCHLINK_TURN_USER=$COUCHLINK_TURN_USER"
    echo "COUCHLINK_TURN_PASS=$COUCHLINK_TURN_PASS"
  } >> "$ENV_FILE"
  echo "==> generated TURN credentials, saved to $ENV_FILE"
fi

PUBLIC_IP="${COUCHLINK_PUBLIC_IP:-$(curl -fsS --max-time 3 ifconfig.me || true)}"
if [[ -n "$PUBLIC_IP" ]] && ! grep -q "^COUCHLINK_TURN_URL=" "$ENV_FILE" 2>/dev/null; then
  echo "COUCHLINK_TURN_URL=turn:$PUBLIC_IP:3478" >> "$ENV_FILE"
fi

RUNTIME_CONF="$(mktemp /tmp/couchlink-turnserver.XXXXXX.conf)"
trap 'rm -f "$RUNTIME_CONF"; upnp_close 3478 udp; upnp_close 3478 tcp' EXIT
sed \
  -e "s/COUCHLINK_TURN_USER/$COUCHLINK_TURN_USER/" \
  -e "s/COUCHLINK_TURN_PASS/$COUCHLINK_TURN_PASS/" \
  "$ROOT/infra/turn/turnserver.conf.example" > "$RUNTIME_CONF"
[[ -n "$PUBLIC_IP" ]] && echo "external-ip=$PUBLIC_IP" >> "$RUNTIME_CONF"

upnp_open 3478 udp "turn"
upnp_open 3478 tcp "turn"

echo "==> starting local TURN relay on :3478 (user=$COUCHLINK_TURN_USER)"
turnserver -c "$RUNTIME_CONF" --no-daemon
