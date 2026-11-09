#!/usr/bin/env bash
# One-command local run of the whole regression gate (mirrors .github/workflows/regression.yml).
#   scripts/ci/regression-local.sh            # everything
#   SKIP_WEB=1 scripts/ci/regression-local.sh # skip npm/vitest
#   BASE=origin/main scripts/ci/regression-local.sh   # base for the weakened-assertions diff
# Heavy cargo steps honour CARGO_BUILD_JOBS / CARGO_TARGET_DIR; wrap in `nice -n 15` if you share the box.
set -euo pipefail
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
BASE="${BASE:-origin/main}"
LOG="$(mktemp -t regression-host.XXXXXX.log)"
step() { printf '\n==== %s ====\n' "$*"; }

step "1/6 thresholds guard + script unit tests"
python3 scripts/ci/regression-guard.py
python3 scripts/ci/test_ci_scripts.py

step "2/6 weakened-assertions check vs $BASE"
if git rev-parse --verify -q "$BASE" >/dev/null; then
  python3 scripts/ci/check-weakened-assertions.py --base "$BASE" --head HEAD
else
  echo "base ref $BASE not found; skipping (git fetch origin main)"
fi

step "3/6 sacred/latency host tests"
cargo test -p couchlink-host --no-fail-fast -- \
  ricardo_playable_ab amazing_latency_ab latency_live_sim link_gov \
  webrtc_peer wan3_math input_photon_budget 2>&1 | tee "$LOG"
python3 scripts/ci/regression-guard.py --test-log "$LOG"

step "4/6 cargo test host/pad/proto/capture-bridge"
cargo test --no-fail-fast -p couchlink-host -p couchlink-pad -p couchlink-proto -p couchlink-capture-bridge

if [ -z "${SKIP_WEB:-}" ]; then
  step "5/6 web sacred tests"
  (cd web && npm ci --no-audit --no-fund && npx vitest run \
    src/presentAge.test.ts src/keyboardMouse.test.ts src/webCodecsCanvas.test.ts \
    src/lowLatencyCanvas.test.ts src/inputPhoton.test.ts)
  step "6/6 whole web suite"
  (cd web && npx vitest run)
fi
echo; echo "regression gate: ALL PASSED"
