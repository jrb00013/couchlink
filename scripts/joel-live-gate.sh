#!/usr/bin/env bash
# Joel live Ricardo gate — real Chrome is S_p50 authority, not Playwright.
#
# 1. Host log scoring (automated)
# 2. You play ~30s in Chrome with a controller wiggling
# 3. Paste scrape JSON → full beat-self gate
#
# Usage:
#   JOIN_URL='https://…' HOST_LOG=/tmp/couchlink-stack.log ./scripts/joel-live-gate.sh
#   ./scripts/joel-live-gate.sh /tmp/ricardo.json   # score pasted scrape
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

HOST_LOG="${HOST_LOG:-}"
if [[ -z "$HOST_LOG" && -f /tmp/couchlink-stack-v16.log ]]; then
  HOST_LOG="/tmp/couchlink-stack-v16.log"
fi
for f in /tmp/couchlink-stack-v{20..10}.log /tmp/couchlink-stack.log; do
  [[ -f "$f" ]] && HOST_LOG="${HOST_LOG:-$f}" && break
done

JOIN_URL="${JOIN_URL:-}"
if [[ -z "$JOIN_URL" && -n "$HOST_LOG" && -f "$HOST_LOG" ]]; then
  JOIN_URL="$(rg -o 'https://[^[:space:]]+trycloudflare\.com/\?s=[^[:space:]]+' "$HOST_LOG" 2>/dev/null | tail -1 || true)"
fi

SCRAPE_ARG="${1:-}"
if [[ -n "$SCRAPE_ARG" && -f "$SCRAPE_ARG" ]]; then
  echo "==> scoring client scrape: $SCRAPE_ARG"
  export HOST_LOG CLIENT_SCRAPE="$SCRAPE_ARG"
  node "$ROOT/scripts/regression-latency-live.mjs"
  exit $?
fi

echo "═══════════════════════════════════════════════════════════════"
echo " JOEL LIVE GATE — beat Ricardo + beat-self (real Chrome S_p50)"
echo "═══════════════════════════════════════════════════════════════"
echo ""
echo "Bars (BEAT_SELF=1 default):"
echo "  push ≥ 90 · shed ≤ 1% · encode ≥ 5000 · paint ≥ 100 · S_p50 ≤ 5 ms"
echo ""

if [[ -n "$HOST_LOG" ]]; then
  echo "==> host log axes (automated)"
  export HOST_LOG HOST_ONLY=1 HOST_WAIT_SEC="${HOST_WAIT_SEC:-20}"
  node "$ROOT/scripts/regression-latency-live.mjs" || true
  echo ""
fi

echo "==> YOUR TURN — real Chrome (not Playwright)"
if [[ -n "$JOIN_URL" ]]; then
  echo "Join URL: $JOIN_URL"
else
  echo "Set JOIN_URL or start stack so we can print the tunnel URL."
fi
echo ""
echo "1. Open join URL in Chrome (HW WebCodecs — prefer-hardware)."
echo "2. Connect a gamepad OR wiggle sticks 30+ seconds in-game/menu."
echo "3. Open DevTools → Console, run:"
echo ""
echo '   copy(JSON.stringify(window.__couchlinkRicardo()))'
echo ""
echo "4. Save clipboard to a file, e.g. /tmp/ricardo.json"
echo "5. Score full gate:"
echo ""
echo "   CLIENT_SCRAPE=/tmp/ricardo.json HOST_LOG=$HOST_LOG \\"
echo "     node scripts/regression-latency-live.mjs"
echo ""
echo "Or one-shot:"
echo "   ./scripts/joel-live-gate.sh /tmp/ricardo.json"
echo ""
echo "Green checklist in scrape JSON:"
echo "  presentMode: webcodecs"
echo "  inputPhoton.watermarkActive: true"
echo "  inputPhoton.sampleCount ≥ 16"
echo "  inputPhoton.surplusP50Ms ≤ 5 (beat-self) or ≤ 45 (Ricardo floor)"
echo "  present.fps ≥ 100 (beat-self) or ≥ 74 (Ricardo)"
echo ""
