#!/usr/bin/env bash
# Block (or re-allow) all viewer input on the host. While locked, the host replaces every viewer
# pad frame with a neutral one before it reaches the virtual controller, so friends can keep
# watching but cannot press anything - nothing can leak onto your desktop while you work in
# another window or virtual desktop. Takes effect within ~150 ms, no host restart needed.
#
#   scripts/input-lock.sh on|lock       block viewer input
#   scripts/input-lock.sh off|unlock    allow it again
#   scripts/input-lock.sh toggle
#   scripts/input-lock.sh status        prints "locked" / "unlocked" (exit 0 = locked, 1 = unlocked)
#
# The lock is a file (same path the host and couchlink-tui use): COUCHLINK_INPUT_LOCK_FILE,
# else ${XDG_RUNTIME_DIR:-/tmp}/couchlink-input.lock.
set -euo pipefail
LOCK="${COUCHLINK_INPUT_LOCK_FILE:-${XDG_RUNTIME_DIR:-/tmp}/couchlink-input.lock}"
case "${1:-status}" in
  on|lock)    mkdir -p "$(dirname "$LOCK")"; echo locked > "$LOCK"; echo "viewer input LOCKED ($LOCK)" ;;
  off|unlock) rm -f "$LOCK"; echo "viewer input unlocked" ;;
  toggle)     if [ -e "$LOCK" ]; then rm -f "$LOCK"; echo "viewer input unlocked"; else mkdir -p "$(dirname "$LOCK")"; echo locked > "$LOCK"; echo "viewer input LOCKED ($LOCK)"; fi ;;
  status)     if [ -e "$LOCK" ]; then echo locked; else echo unlocked; exit 1; fi ;;
  *) echo "usage: $0 on|off|toggle|status" >&2; exit 2 ;;
esac
