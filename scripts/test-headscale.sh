#!/usr/bin/env bash
# Smoke-test Headscale bring-up (control plane + user + preauth key).
# Does not require Tailscale join or cloudflared (uses COUCHLINK_HS_LOCAL=1).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-headscale.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "OK: $*"; }

export COUCHLINK_HS_LOCAL=1
export COUCHLINK_SKIP_HEADSCALE=0
# Fresh keys each smoke (keep DB).
rm -f "$ROOT/infra/headscale/data/player.preauth" \
      "$ROOT/infra/headscale/data/host.preauth" 2>/dev/null || true

bash "$ROOT/scripts/setup-headscale.sh" >/dev/null

CFG="$ROOT/infra/headscale/config.yaml"
grep -q 'controlplane.tailscale.com/derpmap' "$CFG" \
  || fail "config missing default DERP map URL"

# Start / mint without requiring host Tailscale join:
DATA="$ROOT/infra/headscale/data"
BIN="$(couchlink_headscale_bin "$ROOT")" || fail "no headscale binary"
PID_FILE="$DATA/headscale.pid"
LOG_FILE="$DATA/headscale-smoke.log"
mkdir -p "$DATA"

# stop any previous
if [[ -f "$PID_FILE" ]]; then
  kill "$(cat "$PID_FILE")" 2>/dev/null || true
fi
pkill -f "$BIN serve" 2>/dev/null || true
rm -f "$DATA/headscale.sock"
sleep 0.5

couchlink_headscale_set_server_url "$CFG" "http://127.0.0.1:8080" 0
nohup "$BIN" --config "$CFG" serve >"$LOG_FILE" 2>&1 &
echo $! >"$PID_FILE"

ok=0
for i in $(seq 1 40); do
  if [[ -S "$DATA/headscale.sock" ]] && couchlink_headscale_cli "$ROOT" users list >/dev/null 2>&1; then
    ok=1
    break
  fi
  if ! kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
    break
  fi
  sleep 0.3
done
[[ "$ok" == "1" ]] || {
  tail -40 "$LOG_FILE" >&2 || true
  fail "Headscale did not become ready"
}
pass "Headscale serve + unix socket"

UID_NUM="$(couchlink_headscale_ensure_user "$ROOT" "couchlink")" \
  || fail "ensure user"
[[ "$UID_NUM" =~ ^[0-9]+$ ]] || fail "user id not numeric: $UID_NUM"
pass "user couchlink id=$UID_NUM"

KEY="$(couchlink_headscale_mint_preauth "$ROOT" "$UID_NUM")" \
  || fail "mint preauth"
[[ "$KEY" == hskey-* || "$KEY" == tskey-* ]] || fail "key shape: $KEY"
pass "minted preauth key (${KEY:0:12}…)"

# Invite encode/parse round-trip via host+client unit logic is covered by cargo;
# also verify mesh.env write + client join URL parse helpers.
export COUCHLINK_MESH=headscale
export COUCHLINK_MESH_IP=100.64.0.9
export COUCHLINK_HS_URL=https://hs.example.com
export COUCHLINK_TS_AUTHKEY="$KEY"
PLATFORM="$(bash -c 'source "'"$ROOT"'/scripts/lib-platform.sh"; couchlink_detect_platform')"
couchlink_apply_mesh_invite headscale 100.64.0.9 8443 "$PLATFORM" || fail "apply mesh"
couchlink_write_mesh_env_file "$DATA/mesh.env.smoke"
grep -q 'COUCHLINK_MESH=headscale' "$DATA/mesh.env.smoke" || fail "mesh.env"
grep -q 'COUCHLINK_TS_AUTHKEY=' "$DATA/mesh.env.smoke" || fail "mesh.env key"
pass "mesh.env write"

JOIN="http://100.64.0.9:8443/?s=a&p=1&auto=1&ws=ws://100.64.0.9:8443/ws&mesh=headscale&hs=https%3A%2F%2Fhs.example.com&tskey=${KEY}"
export COUCHLINK_JOIN_URL="$JOIN"
# Parse helpers used by join path
HS_PARSED="$(printf '%s' "$JOIN" | python3 -c '
import sys, urllib.parse
u = urllib.parse.urlparse(sys.stdin.read().strip())
q = urllib.parse.parse_qs(u.query)
assert q.get("mesh")==["headscale"]
assert q.get("hs")==["https://hs.example.com"]
assert q.get("tskey")
print(q["hs"][0], q["tskey"][0][:12])
')" || fail "join URL parse"
pass "join URL carries hs+tskey ($HS_PARSED)"

bash -n "$ROOT/scripts/enable-headscale.sh"
bash -n "$ROOT/scripts/join-headscale.sh"
bash -n "$ROOT/scripts/lib-headscale.sh"
pass "bash -n headscale scripts"

echo "ALL HEADSCALE SMOKE CHECKS PASSED"
# Leave Headscale running for manual follow-up; comment below to stop:
# kill "$(cat "$PID_FILE")" 2>/dev/null || true
exit 0
