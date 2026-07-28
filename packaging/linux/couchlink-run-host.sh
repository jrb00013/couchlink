#!/usr/bin/env bash
# Installed as /usr/bin/couchlink-run-host — double-click host launcher.
set -euo pipefail

if ! couchlink-uinput-helper check; then
  if command -v zenity >/dev/null; then
    zenity --error --width=400 --text="Couchlink needs gamepad permissions.\n\n1. Open Apps → Couchlink Host Setup\n2. Enter your password once\n3. Log out and back in\n4. Open Couchlink Host again"
  else
    echo "Run 'Couchlink Host Setup' from the app menu once, then log out/in." >&2
  fi
  exit 1
fi

CONF_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/couchlink"
mkdir -p "$CONF_DIR"
CONF="$CONF_DIR/host.env"
if [[ -f "$CONF" ]]; then
  # shellcheck disable=SC1090
  source "$CONF"
fi

if [[ -z "${COUCHLINK_SESSION_ID:-}" || -z "${COUCHLINK_PIN:-}" ]]; then
  COUCHLINK_SESSION_ID="$(openssl rand -hex 6 2>/dev/null || head -c 6 /dev/urandom | xxd -p)"
  COUCHLINK_PIN="$(printf '%06d' "$((RANDOM % 1000000))")"
  {
    echo "COUCHLINK_SESSION_ID=$COUCHLINK_SESSION_ID"
    echo "COUCHLINK_PIN=$COUCHLINK_PIN"
  } >"$CONF"
  echo "Created session — saved to $CONF"
fi

pkill -x couchlink-signaling 2>/dev/null || true
WEB_ROOT="/usr/share/couchlink/web"
[[ -d "$WEB_ROOT" ]] || WEB_ROOT=""
if [[ -n "$WEB_ROOT" ]]; then
  couchlink-signaling --bind 0.0.0.0:8443 --web-root "$WEB_ROOT" &
else
  couchlink-signaling --bind 0.0.0.0:8443 &
fi
SIG_PID=$!
cleanup() { kill "$SIG_PID" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
sleep 1

echo "Send your friend the join URL printed by the host (or open http://THIS_PC:8443)."
exec couchlink-host \
  --signaling "${COUCHLINK_SIGNALING:-ws://127.0.0.1:8443/ws}" \
  --session-id "$COUCHLINK_SESSION_ID" \
  --pin "$COUCHLINK_PIN" \
  --preset "${COUCHLINK_PRESET:-720p30}"
