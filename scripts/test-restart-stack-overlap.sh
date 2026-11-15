#!/usr/bin/env bash
# Regression for 2026-11-14: overlapping restart-stack must wait / reuse, not
# SIGTERM a half-started run.sh (symptom: "==> shutting down" mid-capture, no join URL).
#
# Safe: uses COUCHLINK_RESTART_DRY_RUN + a private lock file. Never stops the
# live CouchLink / PCSX2 session.
#
#   scripts/test-restart-stack-overlap.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/scripts/restart-stack-for-game.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
LOCK="$TMP/restart.lock"
fail=0

pass() { echo "PASS: $*"; }
bad()  { echo "FAIL: $*" >&2; fail=1; }

# --- static guards (must not regress out of the script) ---------------------
grep -q 'couchlink-restart-stack.lock' "$SCRIPT" \
  || bad "missing default lock path comment/name"
grep -q 'COUCHLINK_FORCE_RESTART' "$SCRIPT" \
  || bad "missing FORCE_RESTART escape hatch"
grep -q 'another restart-stack is in progress' "$SCRIPT" \
  || bad "missing wait message for overlapping restart"
grep -q 'reusing live stack' "$SCRIPT" \
  || bad "missing reuse-after-wait path"
grep -q 'COUCHLINK_RESTART_DRY_RUN' "$SCRIPT" \
  || bad "missing DRY_RUN hook for tests"
grep -q 'COUCHLINK_RESTART_HEALTHY_HOOK' "$SCRIPT" \
  || bad "missing HEALTHY_HOOK for tests"
bash -n "$SCRIPT" && pass "bash -n restart-stack-for-game.sh" || bad "bash -n failed"

# --- behavioral: second invoke waits on flock, does not race into stop ------
(
  exec 9>"$LOCK"
  flock 9
  # Hold long enough that a non-waiting kill-path would have already proceeded.
  sleep 2
) &
holder=$!
sleep 0.2  # let holder grab the lock

start_ms=$(date +%s%3N)
out="$(
  COUCHLINK_RESTART_LOCK="$LOCK" \
  COUCHLINK_RESTART_LOCK_WAIT=10 \
  COUCHLINK_RESTART_DRY_RUN=1 \
  COUCHLINK_FORCE_RESTART=1 \
  bash "$SCRIPT" pcsx2 2>&1
)" || rc=$?
rc=${rc:-0}
end_ms=$(date +%s%3N)
wait "$holder" || true
elapsed=$(( end_ms - start_ms ))

echo "$out" | grep -q 'another restart-stack is in progress' \
  && pass "waiter printed overlap message" \
  || bad "waiter did not print overlap message; out=$out"
echo "$out" | grep -q 'DRY_RUN: acquired restart lock' \
  && pass "waiter acquired lock after holder released" \
  || bad "waiter never reached DRY_RUN acquire; out=$out"
# Must have blocked ~2s on the holder (allow clock skew / scheduling).
if [ "$elapsed" -ge 1500 ]; then
  pass "waiter blocked ${elapsed}ms (>=1500) instead of racing"
else
  bad "waiter returned too fast (${elapsed}ms) — likely skipped flock wait"
fi
[ "$rc" = "0" ] && pass "dry-run exit 0" || bad "dry-run exit $rc"

# --- behavioral: after wait, healthy stack is reused (no tear-down) ---------
# Fake "healthy" via hook; join file present so print_join can succeed without curl failing hard.
printf 'https://example.test/?s=regen' > "$TMP/join-url.txt"
# Point print_join at our temp join file by symlink into /tmp only if safe —
# do NOT clobber a live join URL. Use HEALTHY_HOOK + dry path that exits on reuse
# before print_join needs the real file: the reuse branch calls print_join || true.
out2="$(
  # Hold lock briefly so waiter takes the wait branch.
  ( exec 9>"$LOCK"; flock 9; sleep 1 ) &
  sleep 0.15
  COUCHLINK_RESTART_LOCK="$LOCK" \
  COUCHLINK_RESTART_LOCK_WAIT=10 \
  COUCHLINK_RESTART_HEALTHY_HOOK='true' \
  COUCHLINK_FORCE_RESTART=0 \
  bash "$SCRIPT" pcsx2 2>&1 || true
)"
echo "$out2" | grep -q 'reusing live stack' \
  && pass "healthy-after-wait reuses stack (no stop)" \
  || bad "expected reuse message; out=$out2"
echo "$out2" | grep -q 'stopping old stack' \
  && bad "reuse path still tore down stack" \
  || pass "reuse path did not stop old stack"

# --- behavioral: FORCE_RESTART after wait proceeds to dry-run tear path -----
out3="$(
  ( exec 9>"$LOCK"; flock 9; sleep 1 ) &
  sleep 0.15
  COUCHLINK_RESTART_LOCK="$LOCK" \
  COUCHLINK_RESTART_LOCK_WAIT=10 \
  COUCHLINK_RESTART_HEALTHY_HOOK='true' \
  COUCHLINK_FORCE_RESTART=1 \
  COUCHLINK_RESTART_DRY_RUN=1 \
  bash "$SCRIPT" pcsx2 2>&1
)" || true
echo "$out3" | grep -q 'reusing live stack' \
  && bad "FORCE_RESTART should not reuse" \
  || pass "FORCE_RESTART skipped reuse"
echo "$out3" | grep -q 'DRY_RUN: acquired restart lock' \
  && pass "FORCE_RESTART dry-run acquired lock to continue" \
  || bad "FORCE_RESTART dry-run missing acquire; out=$out3"

if [ "$fail" -ne 0 ]; then
  echo "restart-stack overlap regression: FAILED" >&2
  exit 1
fi
echo "restart-stack overlap regression: ALL PASSED"
