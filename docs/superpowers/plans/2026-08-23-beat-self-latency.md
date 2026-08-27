# Beat-Self Latency Implementation Plan

> **For agentic workers:** Implement task-by-task. Live gate must beat **Ricardo floor** and **frozen self baseline**.

**Goal:** Move from “barely clears Ricardo” (~74.8 push / 84 paint / 7.4ms S) to a **clear self-beat** on the same live stack.

**Architecture:** Push is capped by win-capture `SET_TARGET` fps (preset 60) and dual-send warmup under software WebCodecs. Kill IDR death-spirals, fix handoff averaging honesty, raise encode fps to match capture headroom, and lock regression bars to the frozen self scorecard.

**Tech Stack:** `win_capture` SET_TARGET, `webrtc_peer` keyframe coalesce, `webCodecsCanvas` / `presentAge`, `regression-latency-live.mjs`, `ricardo_gate` / new `self_beat` constants.

## Locked baselines (2026-08-23 live PASS)

| Axis | Ricardo floor | Self-now (frozen) | Beat-self bar |
|------|---------------|-------------------|---------------|
| push fps | ≥74 | 74.8 | **≥90** |
| shed % | ≤3 | 0 | **≤1** |
| encode kbps | ≥5000 | 5000 | **≥5000** (hold) |
| paint fps | ≥74 | 84 | **≥100** |
| S_p50 ms | ≤45 | 7.4 | **≤5.0** |

## Tasks

1. **Honest handoff avg** — `take_handoff_ms` must not divide total `wait_ns` by a capped sample ring length.
2. **Coalesce DC keyframe** — `setup_video_channel` → `request_keyframe_coalesced` (per-peer `LAST_MS` on `self`).
3. **No IDR-on-CPU-backlog** — `decodeBacklogPolicy` skip without requesting IDR unless age is drop/emergency; update tests.
4. **Encode fps headroom** — host `SET_TARGET` fps = `max(preset.fps, COUCHLINK_ENCODE_FPS|COUCHLINK_CAPTURE_FPS)` so 120 capture is not clamped to 60.
5. **Software dual-send thin** — software photon reports a path that keeps RTP for paint but does not permanently sit on full-warmup dual if HW promote is impossible; prefer CLVD-primary + RTP keep-alive only when paint HUD is fed from inbound RTP stats (document trade). *If thin RTP kills paint, keep dual but rely on (4)+(3).*
6. **Regression bars** — live probe + `beats_self` sim gate; `BEAT_SELF=1` default on.
7. **Live verify** — `./scripts/beat-ricardo.sh` must PASS Ricardo **and** self bars.

## Anti-patterns

- Do not request IDR because `decodeQueueSize` is high alone.
- Do not cherry-pick pre-probe host windows.
- Do not claim SHM “fixed” handoff while avg math is wrong.
