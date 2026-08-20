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
_KEEP_TURN_USER="${COUCHLINK_TURN_USER:-}"
_KEEP_TURN_PASS="${COUCHLINK_TURN_PASS:-}"
_KEEP_TURN_EXTERNAL_IP="${COUCHLINK_TURN_EXTERNAL_IP:-}"
_KEEP_SKIP_UPNP="${COUCHLINK_SKIP_UPNP:-}"
# shellcheck disable=SC1090
[[ -f "$ENV_FILE" ]] && source "$ENV_FILE"
[[ -n "$_KEEP_MODE" ]] && COUCHLINK_MODE="$_KEEP_MODE"
[[ -n "$_KEEP_PUBLIC_IP" ]] && COUCHLINK_PUBLIC_IP="$_KEEP_PUBLIC_IP"
[[ -n "$_KEEP_TURN_URL" ]] && COUCHLINK_TURN_URL="$_KEEP_TURN_URL"
[[ -n "$_KEEP_TURN_USER" ]] && COUCHLINK_TURN_USER="$_KEEP_TURN_USER"
[[ -n "$_KEEP_TURN_PASS" ]] && COUCHLINK_TURN_PASS="$_KEEP_TURN_PASS"
[[ -n "$_KEEP_TURN_EXTERNAL_IP" ]] && COUCHLINK_TURN_EXTERNAL_IP="$_KEEP_TURN_EXTERNAL_IP"
[[ -n "$_KEEP_SKIP_UPNP" ]] && COUCHLINK_SKIP_UPNP="$_KEEP_SKIP_UPNP"

MODE="${COUCHLINK_MODE:-online}"
if [[ "$MODE" != "online" ]]; then
  echo "==> local mode — skipping TURN relay"
  exit 0
fi

if ! command -v turnserver >/dev/null; then
  echo "coturn not installed — attempting install"
  # shellcheck disable=SC1091
  source "$ROOT/scripts/lib-platform.sh"
  PLATFORM="$(couchlink_detect_platform)"
  if command -v apt-get >/dev/null; then
    sudo apt-get update -qq && sudo apt-get install -y -qq coturn
  elif [[ "$PLATFORM" == "macos" ]]; then
    BREW="$(couchlink_brew_bin || true)"
    if [[ -n "$BREW" ]]; then
      "$BREW" install coturn
      export PATH="$(couchlink_tool_path "${HOME:-}"):${PATH:-}"
    else
      echo "Install Homebrew (https://brew.sh) then: brew install coturn" >&2
      exit 1
    fi
  else
    echo "Install coturn manually: https://github.com/coturn/coturn" >&2
    exit 1
  fi
  if ! command -v turnserver >/dev/null; then
    echo "coturn install finished but turnserver is still not on PATH" >&2
    exit 1
  fi
fi

# Package install may enable a system coturn on :3478 — stop it so our
# session-scoped config (external-ip + generated creds) can bind the port.
if command -v systemctl >/dev/null 2>&1; then
  if systemctl is-active --quiet coturn 2>/dev/null; then
    echo "==> stopping system coturn service (using session config instead)"
    sudo systemctl stop coturn || true
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
# Invite may use IPv6 or bore.pub while COUCHLINK_PUBLIC_IP stays the WAN IPv4.
TURN_EXTERNAL_IP="${COUCHLINK_TURN_EXTERNAL_IP:-$PUBLIC_IP}"

RUNTIME_CONF="$(mktemp /tmp/couchlink-turnserver.XXXXXX.conf)"
trap 'rm -f "$RUNTIME_CONF"; upnp_close 3478 udp; upnp_close 3478 tcp' EXIT
sed \
  -e "s/COUCHLINK_TURN_USER/$COUCHLINK_TURN_USER/" \
  -e "s/COUCHLINK_TURN_PASS/$COUCHLINK_TURN_PASS/" \
  "$ROOT/infra/turn/turnserver.conf.example" > "$RUNTIME_CONF"
echo "external-ip=$TURN_EXTERNAL_IP" >> "$RUNTIME_CONF"
# Also publish WAN IPv4 when the invite uses a different external (IPv6/bore).
if [[ "$TURN_EXTERNAL_IP" != "$PUBLIC_IP" && "$PUBLIC_IP" != "bore.pub" ]]; then
  echo "external-ip=$PUBLIC_IP" >> "$RUNTIME_CONF"
fi

# Bind the address we advertise.
#
# With no `listening-ip`, this coturn build enumerated IPv4 only — zero IPv6
# sockets even when the machine held a global IPv6. The invite still named that
# address, so a friend behind a strict NAT allocated against a relay that was
# not listening, gathered no `typ relay` candidate, and failed ICE. Naming an
# address we do not bind is the whole bug class; bind it explicitly.
#
# This restriction used to be gated on "external IP looks like IPv6", which
# meant the IPv4 case (the common one) got none of it: coturn fell back to
# its listen-on-everything default and bound all 16 addresses this machine
# had, 12 of them unrelated Docker bridge networks (172.17-172.29.0.1) that
# have nothing to do with the actual WAN path. That is a real, live-observed
# cause of flaky/misrouted TURN allocate responses — a request answered from
# the wrong local interface looks like a malformed response to the client.
# Fixed 2026-08-20: always restrict, not just for IPv6.
#
# Listing any listening-ip disables coturn's listen-on-everything default, so
# loopback and the LAN IPv4 have to be named too or the direct path breaks.
#
# This is passed as `-L` CLI args below, not as `listening-ip=` lines in the
# config file: live-tested 2026-08-20 on this exact coturn build
# (Coturn-4.6.1 'Gorst') and `listening-ip=` in the -c file was silently a
# no-op — verified with -v, the address-discovery banner still enumerated
# all 16 local addresses regardless of what the file said. The identical
# restriction passed as `-L addr` on the command line worked immediately
# (confirmed: exactly the given addresses, no discovery banner at all). The
# config file keeps carrying every other setting (realm/credentials/etc);
# only listening-ip needed the CLI-flag path.
LISTEN_ARGS=(-L 127.0.0.1)
lan4="$(ip -4 route get 1.1.1.1 2>/dev/null | grep -oE 'src [0-9.]+' | awk '{print $2}')"
[[ -n "$lan4" ]] && LISTEN_ARGS+=(-L "$lan4")
if [[ "$TURN_EXTERNAL_IP" == *:* ]] && ip -6 addr show scope global 2>/dev/null \
     | grep -qiF "$TURN_EXTERNAL_IP"; then
  LISTEN_ARGS+=(-L "$TURN_EXTERNAL_IP")
fi
echo "==> TURN binding restricted to loopback + LAN (advertising $TURN_EXTERNAL_IP)"

# Best-effort only — Windows prep / manual forward if the router blocks UPnP.
if [[ "${COUCHLINK_SKIP_UPNP:-}" != "1" ]]; then
  upnp_open 3478 udp "turn" || true
  upnp_open 3478 tcp "turn" || true
fi

echo "==> starting local TURN relay on :3478"
if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
  echo "    user=$COUCHLINK_TURN_USER external-ip=$TURN_EXTERNAL_IP"
fi
# -n keeps coturn in the foreground so run.sh can track the PID.
TURN_LOG="${COUCHLINK_TURN_LOG:-$ROOT/.run/turnserver.log}"
mkdir -p "$(dirname "$TURN_LOG")" 2>/dev/null || true
if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
  exec turnserver -n -c "$RUNTIME_CONF" "${LISTEN_ARGS[@]}"
fi
# Quiet default: stash the banner / STUN dump; keep process in foreground.
exec turnserver -n -c "$RUNTIME_CONF" "${LISTEN_ARGS[@]}" >>"$TURN_LOG" 2>&1
