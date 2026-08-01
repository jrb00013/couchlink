#!/usr/bin/env bash
# Smoke-test PRIME mesh helpers (no root / no real Tailscale required).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "OK: $*"; }

PLATFORM="$(couchlink_detect_platform)"

# --- override path (simulates up mesh) ---
export COUCHLINK_MESH=wireguard
export COUCHLINK_MESH_IP=10.66.0.1
unset COUCHLINK_ICE_IPS || true
unset COUCHLINK_TURN_URL || true
unset COUCHLINK_MESH_NEED_TURN || true

couchlink_try_mesh_online 8443 "$PLATFORM" || fail "try_mesh_online with override"

[[ "${COUCHLINK_MESH_IP}" == "10.66.0.1" ]] || fail "MESH_IP"
[[ "${COUCHLINK_INVITE_SIGNALING}" == "ws://10.66.0.1:8443/ws" ]] || fail "INVITE_SIGNALING=$COUCHLINK_INVITE_SIGNALING"
[[ "${COUCHLINK_SIGNALING}" == "ws://127.0.0.1:8443/ws" ]] || fail "SIGNALING"

if [[ "$PLATFORM" == "wsl" ]]; then
  [[ "${COUCHLINK_MESH_NEED_TURN}" == "1" ]] || fail "WSL should need TURN"
  [[ "${COUCHLINK_TURN_URL}" == "turn:10.66.0.1:3478" ]] || fail "TURN_URL=$COUCHLINK_TURN_URL"
  [[ "${COUCHLINK_TURN_EXTERNAL_IP}" == "10.66.0.1" ]] || fail "TURN_EXTERNAL_IP"
  # Must NOT inject mesh IP into ICE_IPS (dual sole IPv4 crashes webrtc-ice).
  case ",${COUCHLINK_ICE_IPS:-}," in
    *,10.66.0.1,*) fail "WSL must not set mesh IP in ICE_IPS (was ${COUCHLINK_ICE_IPS})" ;;
  esac
  pass "WSL mesh sets TURN on mesh IP without dual ICE NAT"
else
  [[ "${COUCHLINK_MESH_NEED_TURN}" == "0" ]] || fail "native mesh should skip TURN"
  [[ -z "${COUCHLINK_TURN_URL:-}" ]] || fail "native mesh should unset TURN"
  [[ "${COUCHLINK_ICE_IPS}" == "10.66.0.1" ]] || fail "native ICE_IPS=$COUCHLINK_ICE_IPS"
  pass "native mesh clears TURN and sets sole ICE IP"
fi

# --- skip mesh ---
unset COUCHLINK_MESH COUCHLINK_MESH_IP
export COUCHLINK_SKIP_MESH=1
if couchlink_try_mesh_online 8443 "$PLATFORM"; then
  fail "SKIP_MESH should fail try_mesh"
fi
pass "COUCHLINK_SKIP_MESH=1 falls through"
unset COUCHLINK_SKIP_MESH

# --- Tailscale override (paste-link path) ---
export COUCHLINK_MESH=tailscale
export COUCHLINK_MESH_IP=100.64.0.1
unset COUCHLINK_ICE_IPS || true
unset COUCHLINK_TURN_URL || true
couchlink_try_mesh_online 8443 "$PLATFORM" || fail "tailscale override"
[[ "${COUCHLINK_INVITE_SIGNALING}" == "ws://100.64.0.1:8443/ws" ]] || fail "TS INVITE"
[[ "${COUCHLINK_MESH}" == "tailscale" ]] || fail "MESH kind"
pass "Tailscale override sets 100.x invite"
unset COUCHLINK_MESH COUCHLINK_MESH_IP

# --- Headscale override ---
export COUCHLINK_MESH=headscale
export COUCHLINK_MESH_IP=100.64.0.2
export COUCHLINK_HS_URL=https://hs.example.com
export COUCHLINK_TS_AUTHKEY=tskey-auth-test
unset COUCHLINK_ICE_IPS || true
unset COUCHLINK_TURN_URL || true
couchlink_try_mesh_online 8443 "$PLATFORM" || fail "headscale override"
[[ "${COUCHLINK_INVITE_SIGNALING}" == "ws://100.64.0.2:8443/ws" ]] || fail "HS INVITE"
[[ "${COUCHLINK_MESH}" == "headscale" ]] || fail "MESH kind headscale"
pass "Headscale override sets 100.x invite"
unset COUCHLINK_MESH COUCHLINK_MESH_IP COUCHLINK_HS_URL COUCHLINK_TS_AUTHKEY

# --- write_mesh_env_file ---
_tmp_env="$(mktemp)"
export COUCHLINK_MESH=headscale COUCHLINK_MESH_IP=100.64.0.3 COUCHLINK_HS_URL=https://x COUCHLINK_TS_AUTHKEY=k
couchlink_apply_mesh_invite headscale 100.64.0.3 8443 "$PLATFORM" || fail "apply headscale"
couchlink_write_mesh_env_file "$_tmp_env"
grep -q 'COUCHLINK_MESH=headscale' "$_tmp_env" || fail "mesh.env MESH"
grep -q 'COUCHLINK_HS_URL=' "$_tmp_env" || fail "mesh.env HS_URL"
rm -f "$_tmp_env"
unset COUCHLINK_MESH COUCHLINK_MESH_IP COUCHLINK_HS_URL COUCHLINK_TS_AUTHKEY
pass "couchlink_write_mesh_env_file"

# --- setup-wireguard idempotent ---
if command -v wg >/dev/null 2>&1; then
  "$ROOT/scripts/setup-wireguard.sh" >/dev/null
  [[ -f "$ROOT/infra/wireguard/wg0-host.conf" ]] || fail "wg0-host.conf missing"
  git -C "$ROOT" check-ignore -q infra/wireguard/wg0-host.conf \
    || fail "wg0-host.conf must be gitignored"
  git -C "$ROOT" check-ignore -q infra/wireguard/keys/host.key \
    || fail "host.key must be gitignored"
  pass "setup-wireguard + gitignore"
else
  pass "skip setup-wireguard (no wg)"
fi

# --- setup-tailscale --check is non-fatal ---
"$ROOT/scripts/setup-tailscale.sh" --check >/dev/null 2>&1 || true
pass "setup-tailscale --check runs"

# --- bash syntax ---
bash -n "$ROOT/scripts/run.sh"
bash -n "$ROOT/scripts/lib-mesh.sh"
bash -n "$ROOT/scripts/lib-headscale.sh"
bash -n "$ROOT/scripts/setup-headscale.sh"
bash -n "$ROOT/scripts/enable-headscale.sh"
bash -n "$ROOT/scripts/join-headscale.sh"
bash -n "$ROOT/scripts/unblock-firewall.sh"
bash -n "$ROOT/scripts/start-host.sh"
bash -n "$ROOT/scripts/setup-wireguard.sh"
bash -n "$ROOT/scripts/setup-tailscale.sh"
bash -n "$ROOT/install.sh"
pass "bash -n clean"

# --- headscale control-plane smoke (local, no cloudflared) ---
if [[ "${COUCHLINK_SKIP_HEADSCALE_SMOKE:-0}" != "1" ]]; then
  if [[ -x "$ROOT/scripts/test-headscale.sh" ]]; then
    "$ROOT/scripts/test-headscale.sh" || fail "test-headscale.sh"
    pass "test-headscale.sh"
  fi
fi

echo "ALL MESH SMOKE CHECKS PASSED (platform=$PLATFORM)"
