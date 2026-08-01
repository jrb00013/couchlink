#!/usr/bin/env bash
# Generate couchlink WireGuard keys + wg0-host/player.conf (gitignored).
# Does not bring the tunnel up — see docs/WIREGUARD.md / docs/MESH.md.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WG_DIR="$ROOT/infra/wireguard"
KEYS="$WG_DIR/keys"
ROTATE=0

usage() {
  cat <<EOF
usage: $0 [--rotate]

  Generate host + player WireGuard keypairs and conf files under infra/wireguard/.
  Idempotent unless --rotate. Does not run wg-quick (bring-up is manual).

  Env:
    COUCHLINK_PUBLIC_IP   Endpoint host for the player conf (else ifconfig.me, else placeholder)
    COUCHLINK_WG_HOST_IP  Host tunnel address (default 10.66.0.1)
    COUCHLINK_WG_PLAYER_IP Player tunnel address (default 10.66.0.2)
EOF
}

for arg in "$@"; do
  case "$arg" in
    --rotate) ROTATE=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; usage >&2; exit 1 ;;
  esac
done

if ! command -v wg >/dev/null 2>&1; then
  echo "wireguard-tools not found (need \`wg\`)." >&2
  echo "  Linux: sudo apt install wireguard" >&2
  echo "  macOS: brew install wireguard-tools" >&2
  echo "  Or:    COUCHLINK_INSTALL_MESH=1 ./install.sh" >&2
  exit 1
fi

HOST_IP="${COUCHLINK_WG_HOST_IP:-10.66.0.1}"
PLAYER_IP="${COUCHLINK_WG_PLAYER_IP:-10.66.0.2}"
LISTEN_PORT="${COUCHLINK_WG_LISTEN_PORT:-51820}"

ENDPOINT_HOST="${COUCHLINK_PUBLIC_IP:-}"
if [[ -z "$ENDPOINT_HOST" ]]; then
  ENDPOINT_HOST="$(curl -fsS --max-time 5 ifconfig.me 2>/dev/null || true)"
fi
ENDPOINT_HOST="${ENDPOINT_HOST:-HOST_PUBLIC_IP}"

mkdir -p "$KEYS"
chmod 700 "$KEYS"

gen_pair() {
  local name="$1"
  if [[ "$ROTATE" == 1 || ! -f "$KEYS/$name.key" ]]; then
    echo "==> generating $name keypair"
    (umask 077; wg genkey | tee "$KEYS/$name.key" | wg pubkey > "$KEYS/$name.pub")
    chmod 600 "$KEYS/$name.key"
  else
    echo "==> keeping existing $name keypair"
  fi
}

gen_pair host
gen_pair player

HOST_PRIV="$(tr -d '\n' <"$KEYS/host.key")"
HOST_PUB="$(tr -d '\n' <"$KEYS/host.pub")"
PLAYER_PRIV="$(tr -d '\n' <"$KEYS/player.key")"
PLAYER_PUB="$(tr -d '\n' <"$KEYS/player.pub")"

cat >"$WG_DIR/wg0-host.conf" <<EOF
[Interface]
Address = ${HOST_IP}/24
ListenPort = ${LISTEN_PORT}
PrivateKey = ${HOST_PRIV}

[Peer]
PublicKey = ${PLAYER_PUB}
AllowedIPs = ${PLAYER_IP}/32
EOF

cat >"$WG_DIR/wg0-player.conf" <<EOF
[Interface]
Address = ${PLAYER_IP}/24
PrivateKey = ${PLAYER_PRIV}

[Peer]
PublicKey = ${HOST_PUB}
Endpoint = ${ENDPOINT_HOST}:${LISTEN_PORT}
AllowedIPs = ${HOST_IP}/32
PersistentKeepalive = 25
EOF

chmod 600 "$WG_DIR/wg0-host.conf" "$WG_DIR/wg0-player.conf"

echo "==> wrote $WG_DIR/wg0-host.conf"
echo "==> wrote $WG_DIR/wg0-player.conf (Endpoint ${ENDPOINT_HOST}:${LISTEN_PORT})"
echo ""
echo "Next (host):"
echo "  1) Allow UDP ${LISTEN_PORT} inbound (router forward / firewall), or use Tailscale instead"
echo "  2) sudo install -m 600 $WG_DIR/wg0-host.conf /etc/wireguard/wg0.conf"
echo "     sudo wg-quick up wg0"
echo "     (WSL: prefer Windows WireGuard app — import wg0-host.conf; see docs/WIREGUARD.md)"
echo "  3) ./scripts/run.sh host --online   # detects wg0 and prints mesh join URL"
echo ""
echo "Next (friend):"
echo "  1) Copy wg0-player.conf to their machine; import / wg-quick up"
echo "  2) Open the host join URL (http://${HOST_IP}:8443/…) or native client"
echo "  Prefer native client — browser WebCodecs needs https."
