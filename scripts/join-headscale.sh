#!/usr/bin/env bash
# Headless join to host Headscale from invite (hs= + tskey=). No Tailscale Inc account.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"

HS_URL="${COUCHLINK_HS_URL:-}"
TSKEY="${COUCHLINK_TS_AUTHKEY:-}"
JOIN="${COUCHLINK_JOIN_URL:-}"

# Parse from join URL if needed
if [[ -n "$JOIN" ]]; then
  if [[ -z "$HS_URL" ]]; then
    HS_URL="$(printf '%s' "$JOIN" | grep -oE '[?&]hs=[^&]+' | head -1 | sed 's/.*hs=//;s/%3A/:/g;s/%2F/\//g' | python3 -c 'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read().strip()))' 2>/dev/null || true)"
  fi
  if [[ -z "$TSKEY" ]]; then
    TSKEY="$(printf '%s' "$JOIN" | grep -oE '[?&]tskey=[^&]+' | head -1 | sed 's/.*tskey=//' | python3 -c 'import sys,urllib.parse; print(urllib.parse.unquote(sys.stdin.read().strip()))' 2>/dev/null || true)"
  fi
fi

HS_URL="${1:-$HS_URL}"
TSKEY="${2:-$TSKEY}"

if [[ -z "$HS_URL" || -z "$TSKEY" ]]; then
  echo "usage: $0 [hs_url] [tskey]   (or set COUCHLINK_HS_URL + COUCHLINK_TS_AUTHKEY / COUCHLINK_JOIN_URL)" >&2
  exit 1
fi
HS_URL="${HS_URL%/}"

if ! couchlink_find_tailscale_bin >/dev/null 2>&1; then
  echo "==> installing Tailscale client…"
  bash "$ROOT/scripts/setup-tailscale.sh" --ensure || true
fi
BIN="$(couchlink_find_tailscale_bin)" || {
  echo "Tailscale client not found" >&2
  exit 1
}

echo "==> Headscale headless join"
echo "    login-server=$HS_URL"
HOSTN="couchlink-player-$(hostname -s 2>/dev/null || echo friend)"

if [[ "$BIN" == *.exe ]]; then
  "$BIN" up --login-server="$HS_URL" --auth-key="$TSKEY" --accept-dns=false --accept-routes=false \
    --hostname="$HOSTN" \
    || "$BIN" up --login-server="$HS_URL" --authkey="$TSKEY" --accept-dns=false --hostname="$HOSTN"
else
  if command -v sudo >/dev/null 2>&1; then
    sudo "$BIN" up --login-server="$HS_URL" --auth-key="$TSKEY" --accept-dns=false --accept-routes=false \
      --hostname="$HOSTN" \
      || "$BIN" up --login-server="$HS_URL" --auth-key="$TSKEY" --accept-dns=false --hostname="$HOSTN"
  else
    "$BIN" up --login-server="$HS_URL" --auth-key="$TSKEY" --accept-dns=false --hostname="$HOSTN"
  fi
fi

IP="$("$BIN" ip -4 2>/dev/null | head -1 | tr -d ' \r\n' || true)"
echo "==> joined Headscale — local mesh IP=${IP:-unknown}"
exit 0
