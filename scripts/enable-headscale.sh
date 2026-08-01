#!/usr/bin/env bash
# Bring up Headscale + public HTTPS (cloudflared) + host Tailscale join.
# Exports: COUCHLINK_MESH=headscale, COUCHLINK_MESH_IP, COUCHLINK_HS_URL, COUCHLINK_TS_AUTHKEY
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
  sleep 1
}

start_hs() {
  stop_hs
  echo "==> starting Headscale…"
  nohup "$BIN" --config "$CFG" serve >"$LOG_FILE" 2>&1 &
  echo $! >"$PID_FILE"
  local i
  for i in $(seq 1 30); do
    if [[ -S "$DATA/headscale.sock" ]]; then
      return 0
    fi
    sleep 0.5
  done
  echo "Headscale failed to start — see $LOG_FILE" >&2
  tail -20 "$LOG_FILE" >&2 || true
  return 1
}

start_hs

# Public HTTPS URL for clients + embedded DERP (needs TLS).
HS_URL="${COUCHLINK_HS_URL:-}"
if [[ -z "$HS_URL" ]]; then
  echo "==> publishing Headscale via cloudflared (HTTPS for DERP)…"
  if couchlink_start_cloudflared "$ROOT" 8080; then
    HS_URL="$COUCHLINK_CF_URL"
  else
    echo "==> cloudflared failed — set COUCHLINK_HS_URL to a public HTTPS URL" >&2
    exit 1
  fi
fi
HS_URL="${HS_URL%/}"
export COUCHLINK_HS_URL="$HS_URL"

couchlink_headscale_set_server_url "$CFG" "$HS_URL" 1
start_hs

echo "==> Headscale server_url=$HS_URL (embedded DERP on, STUN :3479)"

# User + keys
if ! couchlink_headscale_cli "$ROOT" users list 2>/dev/null | grep -q "$USER_NAME"; then
  couchlink_headscale_cli "$ROOT" users create "$USER_NAME" >/dev/null \
    || couchlink_headscale_cli "$ROOT" users create --name "$USER_NAME" >/dev/null \
    || true
fi

PLAYER_KEY_FILE="$DATA/player.preauth"
HOST_KEY_FILE="$DATA/host.preauth"

mint_key() {
  local out="$1"
  local reusable="${2:-false}"
  local key=""
  # Try modern CLI shapes
  key="$(couchlink_headscale_cli "$ROOT" preauthkeys create \
    --user "$USER_NAME" --reusable --ephemeral --expiration 168h 2>/dev/null \
    | grep -oE '[^ ]{20,}' | tail -1 || true)"
  if [[ -z "$key" ]]; then
    key="$(couchlink_headscale_cli "$ROOT" preauthkeys create -u "$USER_NAME" \
      --reusable --ephemeral 2>/dev/null | grep -oE 'tskey-[^ ]+' | head -1 || true)"
  fi
  if [[ -z "$key" ]]; then
    key="$(couchlink_headscale_cli "$ROOT" preauthkeys create "$USER_NAME" 2>/dev/null \
      | grep -oE 'tskey-[^ ]+' | head -1 || true)"
  fi
  [[ -n "$key" ]] || return 1
  printf '%s' "$key" >"$out"
  chmod 600 "$out"
  printf '%s' "$key"
}

if [[ ! -s "$PLAYER_KEY_FILE" ]]; then
  echo "==> minting player preauth key…"
  mint_key "$PLAYER_KEY_FILE" || {
    echo "failed to mint player preauth key — check headscale CLI" >&2
    exit 1
  }
fi
if [[ ! -s "$HOST_KEY_FILE" ]]; then
  echo "==> minting host preauth key…"
  mint_key "$HOST_KEY_FILE" || cp "$PLAYER_KEY_FILE" "$HOST_KEY_FILE"
fi

PLAYER_KEY="$(tr -d '\r\n' <"$PLAYER_KEY_FILE")"
HOST_KEY="$(tr -d '\r\n' <"$HOST_KEY_FILE")"
export COUCHLINK_TS_AUTHKEY="$PLAYER_KEY"

# Join host Tailscale client to this control plane
TS_BIN="$(couchlink_find_tailscale_bin 2>/dev/null || true)"
if [[ -z "$TS_BIN" ]]; then
  echo "==> Tailscale client missing — running setup-tailscale.sh --ensure"
  bash "$ROOT/scripts/setup-tailscale.sh" --ensure || true
  TS_BIN="$(couchlink_find_tailscale_bin 2>/dev/null || true)"
fi
if [[ -z "$TS_BIN" ]]; then
  echo "Tailscale client required for Headscale mesh" >&2
  exit 1
fi

echo "==> joining host to Headscale ($HS_URL)…"
export TS_AUTHKEY="$HOST_KEY"
if [[ "$TS_BIN" == *.exe ]]; then
  "$TS_BIN" up --login-server="$HS_URL" --auth-key="$HOST_KEY" --accept-dns=false --accept-routes=false \
    --hostname="couchlink-host" 2>/dev/null \
    || "$TS_BIN" up --login-server="$HS_URL" --authkey="$HOST_KEY" --accept-dns=false 2>/dev/null \
    || true
else
  sudo "$TS_BIN" up --login-server="$HS_URL" --auth-key="$HOST_KEY" --accept-dns=false --accept-routes=false \
    --hostname="couchlink-host" \
    || "$TS_BIN" up --login-server="$HS_URL" --auth-key="$HOST_KEY" --accept-dns=false --accept-routes=false \
    || true
fi

sleep 2
MESH_IP=""
MESH_IP="$("$TS_BIN" ip -4 2>/dev/null | head -1 | tr -d ' \r\n' || true)"
if [[ ! "$MESH_IP" =~ ^100\. ]]; then
  # Fallback: ask headscale for nodes
  MESH_IP="$(couchlink_headscale_cli "$ROOT" nodes list -o json 2>/dev/null \
    | grep -oE '100\.[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)"
fi
if [[ ! "$MESH_IP" =~ ^100\. ]]; then
  echo "==> WARN: host has no 100.x yet — finish Tailscale client join, then re-run"
  echo "    login-server=$HS_URL"
  exit 1
fi

PLATFORM="$(couchlink_detect_platform)"
_bind_port="${COUCHLINK_BIND:-0.0.0.0:8443}"
_bind_port="${_bind_port##*:}"
couchlink_apply_mesh_invite headscale "$MESH_IP" "${_bind_port:-8443}" "$PLATFORM"
export COUCHLINK_HS_URL="$HS_URL"
export COUCHLINK_TS_AUTHKEY="$PLAYER_KEY"

MESH_ENV="$DATA/mesh.env"
couchlink_write_mesh_env_file "$MESH_ENV"
# Persist Headscale cloudflared PID so operators can stop it; do not kill on run.sh exit
# (friends still need hs= HTTPS while the host session runs / across restarts).
if [[ -n "${COUCHLINK_TUNNEL_PIDS[*]:-}" ]]; then
  {
    echo "export COUCHLINK_HS_CF_PIDS=$(printf '%q' "${COUCHLINK_TUNNEL_PIDS[*]}")"
    [[ -n "${COUCHLINK_CF_URL:-}" ]] && echo "export COUCHLINK_HS_CF_URL=$(printf '%q' "${COUCHLINK_CF_URL}")"
  } >>"$MESH_ENV"
fi

echo "==> Headscale mesh ready"
echo "    hs=$HS_URL"
echo "    host mesh IP=$MESH_IP"
echo "    player tskey stored in $PLAYER_KEY_FILE (also COUCHLINK_TS_AUTHKEY)"
echo "    env file: $MESH_ENV (sourced by run.sh)"
exit 0
