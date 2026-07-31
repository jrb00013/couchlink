#!/usr/bin/env bash
# Latency regression harness for the browser co-play path.
#
# 1) Contract tests (host playout-delay + web JB gates)
# 2) Optional live probe against a running host (PLAYWRIGHT)
#
# Usage:
#   ./scripts/regression-latency.sh
#   JOIN_URL='http://172.18.223.133:8443/?s=…&p=…&auto=1&ws=…' ./scripts/regression-latency.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "=== 1/2 contract tests (host playout-delay) ==="
cargo test -p couchlink-host latency -- --nocapture

echo ""
echo "=== 2/2 contract tests (web latency gates) ==="
(
  cd "$ROOT/web"
  if [[ ! -d node_modules/vitest ]]; then
    npm install --no-fund --no-audit vitest@3 >/dev/null
  fi
  npx vitest run --reporter=verbose
)

echo ""
echo "=== live browser probe (optional) ==="
JOIN_URL="${JOIN_URL:-}"
if [[ -z "$JOIN_URL" && -f .env.couchlink ]]; then
  # shellcheck disable=SC1091
  source .env.couchlink
  if [[ -n "${COUCHLINK_SIGNALING:-}" && -n "${COUCHLINK_SESSION_ID:-}" && -n "${COUCHLINK_PIN:-}" ]]; then
    SIG="${COUCHLINK_SIGNALING}"
    HTTP="${SIG/ws:/http:}"
    HTTP="${HTTP/wss:/https:}"
    HTTP="${HTTP%/ws}"
    JOIN_URL="${HTTP}/?s=${COUCHLINK_SESSION_ID}&p=${COUCHLINK_PIN}&auto=1&ws=${COUCHLINK_SIGNALING}"
  fi
fi

if [[ -z "$JOIN_URL" ]]; then
  echo "SKIP live probe — set JOIN_URL or COUCHLINK_* in .env.couchlink"
  echo ""
  echo "All contract regressions passed."
  exit 0
fi

echo "JOIN_URL=$JOIN_URL"
node "$ROOT/scripts/regression-latency-live.mjs" "$JOIN_URL"
