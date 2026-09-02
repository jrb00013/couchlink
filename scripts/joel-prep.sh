#!/usr/bin/env bash
# One-shot prep for Joel / beat-self live night.
# Builds release + web if needed, prints the exact host command, copies nothing secret.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export COUCHLINK_PRESET="${COUCHLINK_PRESET:-720p60}"
export COUCHLINK_CAPTURE_FPS="${COUCHLINK_CAPTURE_FPS:-120}"
# Bits/frame of production motion — 10 Mbps@60; pad path untouched.
export COUCHLINK_ENCODE_FPS="${COUCHLINK_ENCODE_FPS:-60}"
export COUCHLINK_BITRATE_KBPS="${COUCHLINK_BITRATE_KBPS:-10000}"
export COUCHLINK_CAPTURE_SOURCE="${COUCHLINK_CAPTURE_SOURCE:-window}"
export COUCHLINK_CAPTURE_WINDOW="${COUCHLINK_CAPTURE_WINDOW:-Marvel - Ultimate Alliance}"
export COUCHLINK_WIN_CAPTURE_FORCE="${COUCHLINK_WIN_CAPTURE_FORCE:-1}"

echo "==> production feel: 1:1 pad + 10Mbps@60 motion (Moonlight-class) + LFW pace"
echo "==> ENCODE_FPS=$COUCHLINK_ENCODE_FPS BITRATE=$COUCHLINK_BITRATE_KBPS"
echo "==> hybrid: full RTP paint; CLVD video off after promote (CLWM tips)"
echo ""

bash "$ROOT/scripts/ensure-host-stack.sh"

echo ""
echo "==> start (full stack in one command):"
echo "    ./scripts/run.sh host --online"
echo ""
echo "==> after join + 30s pad wiggle in Chrome:"
echo "    copy(JSON.stringify(window.__couchlinkRicardo()))"
echo "    ./scripts/joel-live-gate.sh /tmp/ricardo.json"
echo ""
echo "Env pinned for this shell:"
echo "  COUCHLINK_PRESET=$COUCHLINK_PRESET"
echo "  COUCHLINK_CAPTURE_FPS=$COUCHLINK_CAPTURE_FPS"
echo "  COUCHLINK_ENCODE_FPS=$COUCHLINK_ENCODE_FPS"
echo "  COUCHLINK_BITRATE_KBPS=$COUCHLINK_BITRATE_KBPS"
echo "  COUCHLINK_CAPTURE_SOURCE=$COUCHLINK_CAPTURE_SOURCE"
echo "  COUCHLINK_CAPTURE_WINDOW=$COUCHLINK_CAPTURE_WINDOW"
