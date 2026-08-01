#!/usr/bin/env bash
# Live smoke: Headscale control plane + two userspace nodes + ping.
# No Windows Tailscale / no Tailscale Inc login / no blocking sudo.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-headscale.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "OK: $*"; }

echo "==> LIVE Headscale smoke (two-node)"

export COUCHLINK_HS_LOCAL=1
export COUCHLINK_HS_ALLOW_WINDOWS_CLIENT=0
rm -f infra/headscale/data/player.preauth infra/headscale/data/host.preauth \
      infra/headscale/data/mesh.env 2>/dev/null || true

bash scripts/ensure-headscale-client.sh >/dev/null || fail "ensure client"
pass "Linux Headscale client binary present"

bash scripts/enable-headscale.sh || fail "enable-headscale"
# shellcheck disable=SC1091
source infra/headscale/data/mesh.env

[[ "${COUCHLINK_MESH}" == "headscale" ]] || fail "MESH"
[[ -n "${COUCHLINK_HS_URL}" ]] || fail "HS_URL"
[[ -n "${COUCHLINK_TS_AUTHKEY}" ]] || fail "TS_AUTHKEY"
[[ "${COUCHLINK_MESH_IP}" =~ ^100\. ]] || fail "host MESH_IP=${COUCHLINK_MESH_IP:-}"
pass "host on Headscale ip=$COUCHLINK_MESH_IP hs=$COUCHLINK_HS_URL"

# Second node (player) via join-headscale
export COUCHLINK_HS_URL COUCHLINK_TS_AUTHKEY
bash scripts/join-headscale.sh "$COUCHLINK_HS_URL" "$COUCHLINK_TS_AUTHKEY" \
  || fail "player join-headscale"
# Find newest player socket
PLAYER_SOCK="$(ls -1t infra/headscale/data/us-player-*/tailscaled.sock 2>/dev/null | head -1 || true)"
[[ -n "$PLAYER_SOCK" && -S "$PLAYER_SOCK" ]] || fail "player socket missing"
PLAYER_IP="$(couchlink_headscale_userspace_ip "$PLAYER_SOCK")"
[[ "$PLAYER_IP" =~ ^100\. ]] || fail "player IP=$PLAYER_IP"
pass "player on Headscale ip=$PLAYER_IP"

HOST_SOCK="${COUCHLINK_HS_SOCKET:-infra/headscale/data/us-host/tailscaled.sock}"
[[ -S "$HOST_SOCK" ]] || fail "host socket missing"

NODES="$(./.tools/headscale --config infra/headscale/config.yaml nodes list 2>/dev/null || true)"
echo "$NODES" | grep -qE '100\.64\.' || fail "nodes list: $NODES"
pass "headscale nodes list has mesh members"

# Mesh ping host → player
PING_OUT="$(timeout 25 tailscale --socket="$HOST_SOCK" ping -c 3 "$PLAYER_IP" 2>&1 || true)"
echo "$PING_OUT" | grep -qi pong || fail "no pong from player: $PING_OUT"
pass "tailscale ping host→player OK"

JOIN="http://${COUCHLINK_MESH_IP}:8443/?s=smoke&p=123456&auto=1&ws=ws://${COUCHLINK_MESH_IP}:8443/ws&mesh=headscale&hs=$(python3 -c 'import urllib.parse,os; print(urllib.parse.quote(os.environ["COUCHLINK_HS_URL"], safe=""))')&tskey=$(python3 -c 'import urllib.parse,os; print(urllib.parse.quote(os.environ["COUCHLINK_TS_AUTHKEY"], safe=""))')"
echo "$JOIN" | grep -q 'mesh=headscale' || fail "invite"
JOIN="$JOIN" python3 - <<'PY' || fail "parse"
import os, urllib.parse
q = urllib.parse.parse_qs(urllib.parse.urlparse(os.environ["JOIN"]).query)
assert q["mesh"]==["headscale"] and q["hs"] and q["tskey"]
print("invite ok", q["hs"][0], q["tskey"][0][:16]+"…")
PY
pass "invite encode/parse"

# Suppress checks
grep -q 'COUCHLINK_INSTALL_TAILSCALE_CLOUD' install.sh || fail "install opt-in missing"
! grep -E '^\s+bash .*setup-tailscale.sh --ensure' install.sh \
  || fail "install still unconditionally runs setup-tailscale"
pass "Tailscale Inc auto-install suppressed"

echo "ALL LIVE HEADSCALE SMOKE PASSED"
echo "    host=$COUCHLINK_MESH_IP player=$PLAYER_IP hs=$COUCHLINK_HS_URL"
exit 0
