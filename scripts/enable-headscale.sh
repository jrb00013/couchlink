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

# Public HTTPS URL for clients + embedded DERP (needs TLS).
HS_URL="${COUCHLINK_HS_URL:-}"
if [[ -z "$HS_URL" ]]; then
  if [[ "${COUCHLINK_HS_LOCAL:-0}" == "1" ]]; then
    HS_URL="http://127.0.0.1:8080"
    echo "==> COUCHLINK_HS_LOCAL=1 — using $HS_URL (no cloudflared; friends cannot reach this)"
  else
    echo "==> publishing Headscale via cloudflared (HTTPS for DERP)…"
    if couchlink_start_cloudflared "$ROOT" 8080; then
      HS_URL="$COUCHLINK_CF_URL"
    else
      echo "==> cloudflared failed — set COUCHLINK_HS_URL to a public HTTPS URL" >&2
      exit 1
    fi
  fi
fi
HS_URL="${HS_URL%/}"
export COUCHLINK_HS_URL="$HS_URL"

# Enable embedded DERP only for https:// control URLs (Headscale requirement).
ENABLE_EMBEDDED=0
if [[ "$HS_URL" == https://* ]]; then
  ENABLE_EMBEDDED=1
fi
couchlink_headscale_set_server_url "$CFG" "$HS_URL" "$ENABLE_EMBEDDED"
start_hs

echo "==> Headscale server_url=$HS_URL (embedded DERP=$ENABLE_EMBEDDED, STUN :3479)"

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

# Join host to THIS Headscale control plane (never Tailscale Inc).
TS_BIN=""
TS_BIN="$(bash "$ROOT/scripts/ensure-headscale-client.sh")" || TS_BIN=""
if [[ -z "$TS_BIN" || ! -e "$TS_BIN" ]]; then
  echo "Headscale needs the open-source Tailscale *client* binary (not a Tailscale Inc account)." >&2
  echo "    Install (Linux/WSL, no Windows popup): ./scripts/ensure-headscale-client.sh" >&2
  export COUCHLINK_MESH=headscale
  export COUCHLINK_MESH_IP="${COUCHLINK_MESH_IP:-}"
  couchlink_write_mesh_env_file "$DATA/mesh.env"
  exit 1
fi

echo "==> joining host to Headscale control plane ($HS_URL) — NOT login.tailscale.com"
if [[ "$TS_BIN" == *.exe ]]; then
  "$TS_BIN" up --login-server="$HS_URL" --auth-key="$HOST_KEY" --accept-dns=false --accept-routes=false \
    --hostname="couchlink-host" 2>/dev/null \
    || "$TS_BIN" up --login-server="$HS_URL" --authkey="$HOST_KEY" --accept-dns=false 2>/dev/null \
    || true
else
  if command -v sudo >/dev/null 2>&1; then
    sudo "$TS_BIN" up --login-server="$HS_URL" --auth-key="$HOST_KEY" --accept-dns=false --accept-routes=false \
      --hostname="couchlink-host" \
      || "$TS_BIN" up --login-server="$HS_URL" --auth-key="$HOST_KEY" --accept-dns=false --accept-routes=false \
      || true
  else
    "$TS_BIN" up --login-server="$HS_URL" --auth-key="$HOST_KEY" --accept-dns=false --hostname="couchlink-host" || true
  fi
fi

sleep 2
MESH_IP=""
MESH_IP="$("$TS_BIN" ip -4 2>/dev/null | head -1 | tr -d ' \r\n' || true)"
if [[ ! "$MESH_IP" =~ ^100\. ]]; then
  MESH_IP="$(couchlink_headscale_cli "$ROOT" nodes list -o json 2>/dev/null \
    | grep -oE '100\.[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)"
fi
if [[ ! "$MESH_IP" =~ ^100\. ]]; then
  echo "==> WARN: host has no 100.x yet — Headscale is up; finish client join to $HS_URL"
  echo "    sudo $TS_BIN up --login-server=$HS_URL --auth-key=\$(cat $HOST_KEY_FILE)"
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

MESH_ENV="$DATA/mesh.env"
couchlink_write_mesh_env_file "$MESH_ENV"
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
