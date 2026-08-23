#!/usr/bin/env bash
# One-shot: sim gates + live Ricardo hard scrape against a running stack.
# Usage:
#   JOIN_URL='https://…' HOST_LOG=/tmp/couchlink-stack.log ./scripts/beat-ricardo.sh
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
if [[ -z "$JOIN_URL" ]]; then
  echo "FAIL: set JOIN_URL (and optionally HOST_LOG) for the live scrape" >&2
  exit 2
fi

echo "==> live Ricardo hard gate"
export JOIN_URL HOST_LOG
node "$ROOT/scripts/regression-latency-live.mjs" "$JOIN_URL"
echo "==> BEAT RICARDO: OK"
