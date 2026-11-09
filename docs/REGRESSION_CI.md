# Regression CI gate

Workflow: `.github/workflows/regression.yml` (runs on every pull request to any
branch and on push to `main`; superseded runs are cancelled). It sits beside
`ci.yml`, which is unchanged. Linux only: no Windows, no GPU, no live stack.

Source of the "optimals": `docs/superpowers/plans/2026-08-24-quality-bitrate-no-regress-audit.md`
(ef1b605) and `plans/2026-08-26-wednesday-next.md`.

## What it checks

| Job | Check |
|-----|-------|
| `guard` | `scripts/ci/regression-guard.py` reads `ci/regression-baseline.toml` and statically verifies: 1280x720 / 60 fps / 10000 kbps baseline preset, 5000 kbps floor on the 30 fps preset, fps-only ladder holds bitrate and never goes below 45 fps, bitrate cuts only in the `rtp_congested` tier, encoder IDR ceiling 256 KiB, no FEC while RTP is live, Chrome RTCP PLI ignored in hybrid dual, `FRAME_STALE_AFTER` / `RESPAWN_AFTER` (both bridges), `link_gov` `DOWN_TRIGGER_PCT` / `SEVERE_SHED_PCT` / window counts, and that every sacred test still exists as a live `#[test]` (renamed, deleted or `#[ignore]`d fails), plus per-file minimum test counts and web test-file case counts. |
| `guard` (PRs) | `scripts/ci/check-weakened-assertions.py` diffs the PR against its base for `ricardo_*.rs`, `amazing_latency*.rs`, `latency_live_sim/**`, and the test part of `link_gov.rs`. Fails on a lowered `>=`/`MIN_*`, a raised `<=`/`MAX_*`, a changed `==` expectation, a removed assertion, or a removed/renamed/ignored test. Stricter changes pass. |
| `host-sacred` | `cargo test -p couchlink-host --no-fail-fast` filtered to `ricardo_playable_ab`, `amazing_latency_ab`, `latency_live_sim` (incl. `ricardo_gate`), `link_gov`, `webrtc_peer` (incl. `controller_host_tests`), `wan3_math`, `input_photon_budget`; then the guard re-reads the log and requires every sacred test to have run and passed (a filter that matches nothing cannot go green). |
| `crates` | `cargo test --no-fail-fast` for host, pad, proto, capture-bridge. |
| `web` | vitest on `presentAge`, `keyboardMouse`, `webCodecsCanvas`, `lowLatencyCanvas`, `inputPhoton`, then the whole web suite. |

Note: the sacred tests are in-source unit tests (`crates/host/src/...`), not
`crates/host/tests/`, so they are selected by module path.

## Run it locally (one command)

```bash
scripts/ci/regression-local.sh            # SKIP_WEB=1 to skip npm; BASE=origin/main is the diff base
```

Shared box? `CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=$PWD/target nice -n 15 scripts/ci/regression-local.sh`.
Just the fast static part: `python3 scripts/ci/regression-guard.py && python3 scripts/ci/test_ci_scripts.py`.

## Intentionally changing a baseline

1. Edit the value in the code and in `ci/regression-baseline.toml`; update that
   entry's `why` with the commit/doc/incident that justifies it (see the
   5 -> 10 Mbps entry, commit 343ce8e, as the model).
2. If you also touch a protected test assertion, add a line to the PR
   description or any commit message in the PR:
   `regression-baseline-change: <reason>`
   (the weakened-assertions check then reports but allows it).
3. Sacred test renamed on purpose? Update its name in the baseline's `[[sacred]]` list.

The guard itself is covered by `scripts/ci/test_ci_scripts.py` (mutation-style
tests: drifted constants, renamed/ignored sacred tests, loosened/removed
assertions, marker handling, in a throwaway git repo).

## Known divergence from the old docs

`FRAME_STALE_AFTER` is pinned at 1.5 s, not the 4 s some notes imply: issue #67
(08752db) deliberately lowered it (and `RESPAWN_AFTER` 5 s -> 2 s) to shorten
the freeze when win-capture hangs. The guard blocks drift in either direction.

## What this cannot cover

This gate proves the models, constants, governor logic and unit-level behaviour
have not regressed. It does NOT measure real end-to-end latency, push/paint fps,
shed %, encoded bitrate on the wire, or blackouts: those need a live host,
a real browser, and a real link. CI has no GPU, no Windows capture helper, no
RPCS3/PCSX2 and no Chrome-on-WAN. The authority for S_p50 / paint / push is the
live scrape: run the stack, then `scripts/test-live-latency.sh` (and
`window.__couchlinkRicardo()` in real Chrome), scored with
`scripts/joel-live-gate.sh` / `scripts/regression-latency.sh`. A green
regression gate is necessary, not sufficient, before claiming "beats Ricardo".
Static pattern checks can also be satisfied by code that is dead or moved; they
catch accidental drift, not a determined rewrite.
