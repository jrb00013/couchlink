#!/usr/bin/env bash
# Run a local coturn TURN relay so friends behind symmetric NAT / CGNAT can still
# join — STUN alone (webrtc_peer.rs) doesn't punch through those. Auto-generates
# credentials into .env.couchlink on first run.
#
# Only used for --online host sessions (run.sh skips this script in --local mode).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENV_FILE="$ROOT/.env.couchlink"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-upnp.sh"

_KEEP_MODE="${COUCHLINK_MODE:-}"
_KEEP_PUBLIC_IP="${COUCHLINK_PUBLIC_IP:-}"
_KEEP_TURN_URL="${COUCHLINK_TURN_URL:-}"
# shellcheck disable=SC1090
[[ -f "$ENV_FILE" ]] && source "$ENV_FILE"
[[ -n "$_KEEP_MODE" ]] && COUCHLINK_MODE="$_KEEP_MODE"
[[ -n "$_KEEP_PUBLIC_IP" ]] && COUCHLINK_PUBLIC_IP="$_KEEP_PUBLIC_IP"
[[ -n "$_KEEP_TURN_URL" ]] && COUCHLINK_TURN_URL="$_KEEP_TURN_URL"

MODE="${COUCHLINK_MODE:-online}"
if [[ "$MODE" != "online" ]]; then
  echo "==> local mode — skipping TURN relay"
  exit 0
fi

if ! command -v turnserver >/dev/null; then
  echo "coturn not installed — installing (needs sudo)"
  if command -v apt-get >/dev/null; then
    sudo apt-get update -qq && sudo apt-get install -y -qq coturn
  else
    echo "Install coturn manually: https://github.com/coturn/coturn"
    exit 1
  fi
fi

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
if [[ -z "$PUBLIC_IP" ]]; then
  echo "TURN needs a public IP — set COUCHLINK_PUBLIC_IP or fix outbound HTTPS to ifconfig.me" >&2
  exit 1
fi
COUCHLINK_TURN_URL="${COUCHLINK_TURN_URL:-turn:$PUBLIC_IP:3478}"

RUNTIME_CONF="$(mktemp /tmp/couchlink-turnserver.XXXXXX.conf)"
trap 'rm -f "$RUNTIME_CONF"; upnp_close 3478 udp; upnp_close 3478 tcp' EXIT
sed \
  -e "s/COUCHLINK_TURN_USER/$COUCHLINK_TURN_USER/" \
  -e "s/COUCHLINK_TURN_PASS/$COUCHLINK_TURN_PASS/" \
  "$ROOT/infra/turn/turnserver.conf.example" > "$RUNTIME_CONF"
echo "external-ip=$PUBLIC_IP" >> "$RUNTIME_CONF"

upnp_open 3478 udp "turn"
upnp_open 3478 tcp "turn"

echo "==> starting local TURN relay on :3478 (user=$COUCHLINK_TURN_USER external-ip=$PUBLIC_IP)"
turnserver -c "$RUNTIME_CONF" --no-daemon
