#!/usr/bin/env bash
# Headless join to host Headscale from invite (hs= + tskey=).
# No Tailscale Inc account. No Windows Tailscale MSI/UAC popup.
# Uses userspace networking when system tailscaled needs root.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-headscale.sh"

HS_URL="${COUCHLINK_HS_URL:-}"
TSKEY="${COUCHLINK_TS_AUTHKEY:-}"
JOIN="${COUCHLINK_JOIN_URL:-}"

if [[ -n "$JOIN" ]]; then
  if [[ -z "$HS_URL" ]]; then
    HS_URL="$(printf '%s' "$JOIN" | python3 -c '
import sys, urllib.parse
u = urllib.parse.urlparse(sys.stdin.read().strip())
q = urllib.parse.parse_qs(u.query)
print((q.get("hs") or [""])[0])
' 2>/dev/null || true)"
  fi
  if [[ -z "$TSKEY" ]]; then
    TSKEY="$(printf '%s' "$JOIN" | python3 -c '
import sys, urllib.parse
u = urllib.parse.urlparse(sys.stdin.read().strip())
q = urllib.parse.parse_qs(u.query)
print((q.get("tskey") or [""])[0])
' 2>/dev/null || true)"
  fi
fi

HS_URL="${1:-$HS_URL}"
TSKEY="${2:-$TSKEY}"

if [[ -z "$HS_URL" || -z "$TSKEY" ]]; then
  echo "usage: $0 [hs_url] [tskey]   (or set COUCHLINK_HS_URL + COUCHLINK_TS_AUTHKEY / COUCHLINK_JOIN_URL)" >&2
  exit 1
fi
HS_URL="${HS_URL%/}"

bash "$ROOT/scripts/ensure-headscale-client.sh" >/dev/null || {
  echo "Headscale client binary not found — run: ./scripts/ensure-headscale-client.sh" >&2
  exit 1
}

NAME="player-$(hostname -s 2>/dev/null || echo friend)-$$"
echo "==> Headscale headless join (NOT Tailscale Inc)"
echo "    login-server=$HS_URL"

SOCK="$(couchlink_headscale_userspace_up "$ROOT" "$NAME" "$HS_URL" "$TSKEY")" || {
  echo "Headscale join failed — check login-server reachability and key" >&2
  exit 1
}
IP="$(couchlink_headscale_userspace_ip "$SOCK" || true)"
echo "==> joined Headscale — local mesh IP=${IP:-unknown}"
echo "    socket=$SOCK"
exit 0
