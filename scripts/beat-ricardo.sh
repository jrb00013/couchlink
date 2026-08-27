#!/usr/bin/env bash
# One-shot: sim gates + host log + Joel live Chrome instructions.
# Playwright is NOT the S_p50 authority — use joel-live-gate.sh for full pass.
#
# Usage:
#   JOIN_URL='https://…' HOST_LOG=/tmp/couchlink-stack.log ./scripts/beat-ricardo.sh
#   CLIENT_SCRAPE=/tmp/ricardo.json ./scripts/beat-ricardo.sh  # after Joel scrape
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

./scripts/test-live-latency.sh

JOIN_URL="${JOIN_URL:-}"
HOST_LOG="${HOST_LOG:-}"
if [[ -z "$JOIN_URL" && -f /tmp/couchlink-stack-v10.log ]]; then
  JOIN_URL="$(rg -o 'https://[^[:space:]]+trycloudflare\.com/\?s=[^[:space:]]+' /tmp/couchlink-stack-v10.log | tail -1 || true)"
  HOST_LOG="${HOST_LOG:-/tmp/couchlink-stack-v10.log}"
fi

if [[ -n "${CLIENT_SCRAPE:-}" && -f "$CLIENT_SCRAPE" ]]; then
  echo "==> full live gate (host + Joel scrape)"
  export JOIN_URL HOST_LOG CLIENT_SCRAPE
  node "$ROOT/scripts/regression-latency-live.mjs"
  echo "==> BEAT RICARDO: OK"
  exit 0
fi

echo "==> host log gate (automated)"
export JOIN_URL HOST_LOG HOST_ONLY=1
node "$ROOT/scripts/regression-latency-live.mjs"

echo ""
echo "==> host axes OK — finish in real Chrome (see scripts/joel-live-gate.sh)"
export JOIN_URL HOST_LOG
exec "$ROOT/scripts/joel-live-gate.sh"
