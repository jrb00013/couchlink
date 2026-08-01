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
[[ "${COUCHLINK_ICE_IPS}" == *"10.66.0.1"* ]] || fail "ICE_IPS=$COUCHLINK_ICE_IPS"

if [[ "$PLATFORM" == "wsl" ]]; then
  [[ "${COUCHLINK_MESH_NEED_TURN}" == "1" ]] || fail "WSL should need TURN"
  [[ "${COUCHLINK_TURN_URL}" == "turn:10.66.0.1:3478" ]] || fail "TURN_URL=$COUCHLINK_TURN_URL"
  [[ "${COUCHLINK_TURN_EXTERNAL_IP}" == "10.66.0.1" ]] || fail "TURN_EXTERNAL_IP"
  pass "WSL mesh sets TURN on mesh IP"
else
  [[ "${COUCHLINK_MESH_NEED_TURN}" == "0" ]] || fail "native mesh should skip TURN"
  [[ -z "${COUCHLINK_TURN_URL:-}" ]] || fail "native mesh should unset TURN"
  pass "native mesh clears TURN"
fi

# --- skip mesh ---
unset COUCHLINK_MESH COUCHLINK_MESH_IP
export COUCHLINK_SKIP_MESH=1
if couchlink_try_mesh_online 8443 "$PLATFORM"; then
  fail "SKIP_MESH should fail try_mesh"
fi
pass "COUCHLINK_SKIP_MESH=1 falls through"
unset COUCHLINK_SKIP_MESH

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
bash -n "$ROOT/scripts/start-host.sh"
bash -n "$ROOT/scripts/setup-wireguard.sh"
bash -n "$ROOT/scripts/setup-tailscale.sh"
pass "bash -n clean"

echo "ALL MESH SMOKE CHECKS PASSED (platform=$PLATFORM)"
