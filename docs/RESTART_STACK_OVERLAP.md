# Restart-stack overlap lock (2026-11-14)

## Incident

Bringing CouchLink up from a `live-main` worktree while a second
`scripts/restart-stack-for-game.sh` ran from another checkout caused the
first `run.sh host` to exit mid-boot (`==> shutting down` right after
`ensuring Windows capture binary is built…`). No join URL was written.
Root cause: the second script's "stopping old stack" loop SIGTERM'd the
still-starting `bash ./scripts/run.sh host`, which fired the EXIT cleanup
trap.

## Fix

`scripts/restart-stack-for-game.sh` takes an exclusive flock on
`/tmp/couchlink-restart-stack.lock` (override: `COUCHLINK_RESTART_LOCK`).
A concurrent invoke waits (default 180s) and, if the stack is already
healthy, **reuses** the live join URL instead of tearing down. Set
`COUCHLINK_FORCE_RESTART=1` to force a tear-down after acquiring the lock.

Regression: `scripts/test-restart-stack-overlap.sh` (dry-run; does not
touch a live session).

## Live snappy check (MUA / PCSX2 session, read-only)

Same host log `/tmp/couchlink-stack-20261114-233920.log`:

| Window | FPS avg | Relay avg | Drop |
|--------|---------|-----------|------|
| Early baseline (samples 50–200) | 54.0 | 4.3 ms | 0% |
| 20‑min monitor (2026-11-15 13:48–14:06) | 55.2 | 4.2 ms | ~0.9% avg, max 10% blips |

Verdict: **AS_GOOD_OR_BETTER** vs early baseline. No `client rtt ≥250ms`
info lines during the monitor window.
