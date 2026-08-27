#!/usr/bin/env bash
# Live-latency regression — closest thing to Joel's drawer scrape without a browser.
# Run before release: ./scripts/test-live-latency.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "==> host: latency live sim + AB gates"
cargo test -p couchlink-host -- latency_live_sim::ricardo_gate 2>&1
cargo test -p couchlink-host -- ricardo_playable 2>&1
cargo test -p couchlink-host -- amazing_latency::tests::mediocre 2>&1
cargo test -p couchlink-host link_gov:: 2>&1
cargo test -p couchlink-host webrtc_peer::controller_host_tests::trickle 2>&1
cargo test -p couchlink-host webrtc_peer::controller_host_tests::two_peer 2>&1
cargo test -p couchlink-host webrtc_peer::controller_host_tests::video_dc_buffer 2>&1

echo "==> web: presentAge + kbm + webCodecs diagnosis"
cd "$ROOT/web"
npm test -- --run presentAge keyboardMouse webCodecsCanvas lowLatencyCanvas inputPhoton 2>&1

echo "==> live-latency regression: OK"
echo "    host $(cargo test -p couchlink-host latency_live_sim 2>&1 | rg -c '^test .* \.\.\. ok$' || echo 0) sim tests"
