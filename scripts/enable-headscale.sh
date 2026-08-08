#!/usr/bin/env bash
# Bring up Headscale + public HTTPS (cloudflared) + host Tailscale join.
# Exports via infra/headscale/data/mesh.env:
#   COUCHLINK_MESH=headscale, COUCHLINK_MESH_IP, COUCHLINK_HS_URL, COUCHLINK_TS_AUTHKEY
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-online-tunnel.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-headscale.sh"

HS_DIR="$ROOT/infra/headscale"
DATA="$HS_DIR/data"
CFG="$HS_DIR/config.yaml"
PID_FILE="$DATA/headscale.pid"
LOG_FILE="$DATA/headscale.log"
USER_NAME="${COUCHLINK_HS_USER:-couchlink}"

mkdir -p "$DATA"

if [[ "${COUCHLINK_SKIP_HEADSCALE:-0}" == "1" ]]; then
  echo "==> COUCHLINK_SKIP_HEADSCALE=1 — skipping Headscale"
  exit 1
fi

bash "$ROOT/scripts/setup-headscale.sh"

BIN="$(couchlink_headscale_bin "$ROOT")" || {
  echo "headscale binary missing" >&2
  exit 1
}

stop_hs() {
  if [[ -f "$PID_FILE" ]]; then
    kill "$(cat "$PID_FILE")" 2>/dev/null || true
    rm -f "$PID_FILE"
  fi
  pkill -f "$BIN serve" 2>/dev/null || true
  # socket may linger briefly
  rm -f "$DATA/headscale.sock" 2>/dev/null || true
  sleep 1
}

start_hs() {
  stop_hs
  echo "==> starting Headscale…"
  nohup "$BIN" --config "$CFG" serve >"$LOG_FILE" 2>&1 &
  echo $! >"$PID_FILE"
  local i
  for i in $(seq 1 40); do
    if [[ -S "$DATA/headscale.sock" ]]; then
      # CLI must answer before we declare ready
      if couchlink_headscale_cli "$ROOT" users list >/dev/null 2>&1; then
        return 0
      fi
    fi
    # Fail fast if process died
    if [[ -f "$PID_FILE" ]] && ! kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
      break
    fi
    sleep 0.4
  done
  echo "Headscale failed to start — see $LOG_FILE" >&2
  tail -30 "$LOG_FILE" >&2 || true
  return 1
}

start_hs

# Friend-reachable control URL for invite (hs=).
# Prefer public IP:8080 — Cloudflare quick tunnels break Tailscale Noise register (HTTP 400).
# Host always joins via http://127.0.0.1:8080 on the same machine.
HS_URL="${COUCHLINK_HS_URL:-}"
if [[ -z "$HS_URL" ]]; then
  if [[ "${COUCHLINK_HS_LOCAL:-0}" == "1" ]]; then
    HS_URL="http://127.0.0.1:8080"
    echo "==> COUCHLINK_HS_LOCAL=1 — using $HS_URL (friends need reachability to this host)"
  elif [[ "${COUCHLINK_HS_USE_CLOUDFLARED:-0}" == "1" ]]; then
    echo "==> COUCHLINK_HS_USE_CLOUDFLARED=1 — publishing Headscale (may break Noise; prefer public IP)"
    if couchlink_start_cloudflared "$ROOT" 8080; then
      HS_URL="$COUCHLINK_CF_URL"
    else
      echo "==> cloudflared failed" >&2
      exit 1
    fi
  else
    # Self-hosted, zero-config friend reachability: advertise the host's public
    # IPv6 as the control-plane URL. IPv6 has no NAT, so a global address is an
    # always-on inbound path — the functional equivalent of a router port forward
    # with no Spectrum app change, no relay, and no cloud. Windows portproxy
    # (v6tov4) maps the inbound to WSL's headscale. Only cost: the friend needs
    # working IPv6 (mobile + most ISPs have it). Override anytime with
    # COUCHLINK_HS_URL.
    PUBLIC_V6="$(couchlink_read_public_ipv6 2>/dev/null || true)"
    if [[ -n "$PUBLIC_V6" && "$PUBLIC_V6" == *:* ]]; then
      HS_URL="http://[${PUBLIC_V6}]:8080"
      echo "==> Headscale invite URL $HS_URL (public IPv6 — no router forward, no relay)"
      echo "    friend needs IPv6; override: COUCHLINK_HS_URL=https://hs.example.com"
    else
      PUBLIC_IP="${COUCHLINK_PUBLIC_IP:-}"
      if [[ -z "$PUBLIC_IP" ]]; then
        PUBLIC_IP="$(curl -fsS --max-time 5 ifconfig.me 2>/dev/null || true)"
      fi
      if [[ -n "$PUBLIC_IP" ]]; then
        HS_URL="http://${PUBLIC_IP}:8080"
        echo "==> Headscale invite URL $HS_URL (open TCP 8080 / UPnP for friends)"
        echo "    override: COUCHLINK_HS_URL=https://hs.example.com"
      else
        HS_URL="http://127.0.0.1:8080"
        echo "==> no public IP — using $HS_URL (set COUCHLINK_HS_URL for friends)"
      fi
    fi
  fi
fi
HS_URL="${HS_URL%/}"
export COUCHLINK_HS_URL="$HS_URL"
LOCAL_LOGIN="http://127.0.0.1:8080"

# Embedded DERP needs HTTPS server_url. Keep Tailscale DERP map for http:// invites.
ENABLE_EMBEDDED=0
if [[ "$HS_URL" == https://* ]]; then
  ENABLE_EMBEDDED=1
fi
couchlink_headscale_set_server_url "$CFG" "$HS_URL" "$ENABLE_EMBEDDED"
start_hs

echo "==> Headscale server_url=$HS_URL (embedded DERP=$ENABLE_EMBEDDED, STUN :34790)"

echo "==> ensuring user '$USER_NAME'…"
USER_ID="$(couchlink_headscale_ensure_user "$ROOT" "$USER_NAME")" || {
  echo "failed to create/list Headscale user '$USER_NAME'" >&2
  exit 1
}
echo "    user id=$USER_ID"

PLAYER_KEY_FILE="$DATA/player.preauth"
HOST_KEY_FILE="$DATA/host.preauth"

mint_to_file() {
  local out="$1"
  local key=""
  key="$(couchlink_headscale_mint_preauth "$ROOT" "$USER_ID")" || return 1
  printf '%s' "$key" >"$out"
  chmod 600 "$out"
  printf '%s' "$key"
}

if [[ ! -s "$PLAYER_KEY_FILE" ]]; then
  echo "==> minting player preauth key…"
  mint_to_file "$PLAYER_KEY_FILE" >/dev/null || {
    echo "failed to mint player preauth key — check headscale CLI" >&2
    exit 1
  }
fi
if [[ ! -s "$HOST_KEY_FILE" ]]; then
  echo "==> minting host preauth key…"
  mint_to_file "$HOST_KEY_FILE" >/dev/null || cp "$PLAYER_KEY_FILE" "$HOST_KEY_FILE"
fi

PLAYER_KEY="$(tr -d '\r\n' <"$PLAYER_KEY_FILE")"
HOST_KEY="$(tr -d '\r\n' <"$HOST_KEY_FILE")"
export COUCHLINK_TS_AUTHKEY="$PLAYER_KEY"

# Ensure open-source client binary exists (Linux preferred; no Windows UAC).
bash "$ROOT/scripts/ensure-headscale-client.sh" >/dev/null || {
  echo "Headscale client binary missing — ./scripts/ensure-headscale-client.sh" >&2
  export COUCHLINK_MESH=headscale
  couchlink_write_mesh_env_file "$DATA/mesh.env"
  exit 1
}

echo "==> joining host to Headscale via userspace client ($LOCAL_LOGIN) — NOT login.tailscale.com"
US_SOCK=""
US_SOCK="$(couchlink_headscale_userspace_up "$ROOT" host "$LOCAL_LOGIN" "$HOST_KEY")" || {
  echo "==> userspace join failed — see infra/headscale/data/us-host/tailscaled.log" >&2
  export COUCHLINK_MESH=headscale
  export COUCHLINK_HS_URL="$HS_URL"
  export COUCHLINK_TS_AUTHKEY="$PLAYER_KEY"
  couchlink_write_mesh_env_file "$DATA/mesh.env"
  exit 1
}

sleep 1
MESH_IP="$(couchlink_headscale_userspace_ip "$US_SOCK" || true)"
if [[ ! "$MESH_IP" =~ ^100\. ]]; then
  MESH_IP="$(couchlink_headscale_cli "$ROOT" nodes list -o json 2>/dev/null \
    | grep -oE '100\.[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)"
fi
if [[ ! "$MESH_IP" =~ ^100\. ]]; then
  echo "==> WARN: host has no 100.x yet"
  export COUCHLINK_MESH=headscale
  export COUCHLINK_HS_URL="$HS_URL"
  export COUCHLINK_TS_AUTHKEY="$PLAYER_KEY"
  couchlink_write_mesh_env_file "$DATA/mesh.env"
  exit 1
fi

PLATFORM="$(couchlink_detect_platform)"
_bind_port="${COUCHLINK_BIND:-0.0.0.0:8443}"
_bind_port="${_bind_port##*:}"
couchlink_apply_mesh_invite headscale "$MESH_IP" "${_bind_port:-8443}" "$PLATFORM"
export COUCHLINK_HS_URL="$HS_URL"
export COUCHLINK_TS_AUTHKEY="$PLAYER_KEY"
export COUCHLINK_HS_SOCKET="$US_SOCK"

MESH_ENV="$DATA/mesh.env"
couchlink_write_mesh_env_file "$MESH_ENV"
{
  echo "export COUCHLINK_HS_SOCKET=$(printf '%q' "$US_SOCK")"
  echo "export COUCHLINK_HS_LOCAL_LOGIN=$(printf '%q' "$LOCAL_LOGIN")"
} >>"$MESH_ENV"

echo "==> Headscale mesh ready"
echo "    hs=$HS_URL (friend join / invite)"
echo "    host mesh IP=$MESH_IP (userspace client)"
echo "    player tskey stored in $PLAYER_KEY_FILE (also COUCHLINK_TS_AUTHKEY)"
echo "    env file: $MESH_ENV (sourced by run.sh)"
exit 0
