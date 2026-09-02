# Felt Input-Lag Reduction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut the gap between a friend's thumb (or key) and the pixels they see, without re-freezing the picture, shedding the stream, or touching PCSX2 / `ds-vhid`.

**Architecture:** Do not retune bitrate, chroma, or the governor to "fix lag." Stamp a host-clock capture time into every CLVD access unit, echo it on the pad channel at paint, and log p50/p95 `age` so every later change is falsifiable. Then expedite the *next* encoded frame when a pad arrives (wake-on-input). Only after those two numbers exist, measure WGC arrival spread and the Hyper-V hop; phase-lock or replace vsock only if the instrument says the wait is still there.

**Tech Stack:** Existing CLVD v2 (`crates/proto/src/video_frame.rs` + `web/src/clvd.ts`), pad DataChannel (`CLPD` + JSON `PadFeedback`), Hyper-V vsock capture (`hyperv:9877`), Media Foundation `CODECAPI_AVLowLatencyMode`, host `select!` loop in `crates/host/src/main.rs`.

**Source PR:** [PR #42](https://github.com/jrb00013/couchlink/pull/42) (`docs/COLOR_444_HD_LOW_LATENCY_AUDIT.md`, 2026-08-21). That audit's *color* ladder is a different plan. This document takes only its lag levers and updates them against what shipped on 2026-08-22.

Companions: `docs/LATENCY.md`, `docs/OPTIMIZATION_PLAN.md`, `docs/superpowers/plans/2026-08-06-full-latency-optimization-plan.md`, `docs/superpowers/plans/2026-08-06-latency-next-session.md`, `docs/superpowers/plans/2026-08-22-browser-audio.md`.

## Status

| Task | State |
|------|-------|
| 0. This plan (risk + test gates) | **This commit** — implementation not started |
| 1. Glass-to-glass `age` (CLVD stamp + pad echo) | Not started — **blocking** |
| 2. Surface win-capture arrived/sent into `host_stats` | Not started |
| 3. Wake-on-input (expedite next frame after pad) | Not started |
| 4. Measure WGC arrival spread (gate for phase-lock) | Not started |
| 5. Phase-lock WGC→encode **only if** Task 4 says so | Gated |
| 6. Measure Hyper-V hop; shm only if hop is still material | Gated |
| 7. Live regression + 3-viewer integration | Not started |

## What is still useful from PR #42 (and what is not)

PR #42's §2c ranked lag levers. Current tree, 2026-08-22:

| Lever from PR #42 | Keep? | Why |
|---|---|---|
| Glass-to-glass `age` stamp | **Yes — do first** | Still unbuilt. CLVD header is 18 bytes, no timestamp. Every latency number we quote is still `getStats()` / vibes. |
| Wake-on-input | **Yes — do second** | Still unbuilt. This is felt lag (thumb → pixels), not transit. |
| Keep `AVLowLatencyMode` + short GOP | **Yes — never regress** | Already set in `mf_encoder.rs`. Any change that drops this flag is a failed preset, not a quality win. |
| Hyper-V / shared-memory handoff | **Measure, don't rebuild** | `hyperv:9877` already replaced TCP vSwitch for the live path. Shared-memory is only justified if Task 6 measures a hop that still matters. |
| Phase-lock capture to composition | **Measure, then maybe** | Host already polls pre-encoded frames at **2 ms** (`main.rs`). The remaining metronome is WGC → `next_submit` inside `win_capture.rs`. Do not phase-lock the host tick. |
| Single present path (CLVD *or* RTP) | **No — inverted** | Cutting RTP after WebCodecs first paint froze the last picture on a lost CLVD IDR (`b26cf34`). `path_flags(PATH_WEBCODECS) = (true, true)`. Dual-send is the safety net. |
| True H.264 4:4:4 / Hi444PP / HEVC 444 | **No — different wall** | PR #42's own conclusion: consumer MF is NV12. Chroma work must not lengthen encode or grow the jitter buffer. Out of scope here. |
| Intra-refresh via generic MF | **No — already refuted** | `2026-08-06-latency-next-session.md` §2.2: no `CODECAPI_AVEncVideoIntraRefreshMode` on this ICodecAPI surface. |
| MF slice encode | **Not this series** | API exists; needs a live decode watch and a CLVD partial-frame wire change. Revisit only after `age` exists so we can prove it. |

## Global Constraints

- **Do not close PCSX2 / the game** to implement or verify. Host / win-capture / signaling restarts are allowed; `couchlink-ds-vhid` is not killed mid-session.
- **Do not cut RTP** after WebCodecs first paint. `path_flags` stays `(true, true)` for `PATH_WEBCODECS` / `PATH_UNKNOWN`.
- **Do not feed lag work into `link_gov`.** Expedite, age, and hop counters are not sheds. Governor stays video-drop-only (`DOWN_TRIGGER_PCT=8`, `UP_AFTER_CLEAN_WINDOWS=8`).
- **Do not put age echoes or expedite signals on `video_dc`.** Pad channel only. CLVD stays Annex-B + FEC.
- **Do not implement 4:4:4, software x264, or B-frames** in this series.
- **Do not mix** with uncommitted roster / KBM viz / `host_pad` work. This branch is the plan; implementation PRs are their own commits.
- Pads are created **on join**, not `prebind_all()` for empty seats.
- Capture stays attached to the **PCSX2 process**, not the desktop / game-title-only.
- Off switches: `COUCHLINK_WAKE_ON_INPUT=0` disables Task 3; age instrumentation has no off switch (it is bytes + a log line).
- Failure mode is always **same picture + same pads**, never "Waiting for host offer" / wedged `HyperVBridge::connect`.

## Measured facts (do not re-derive)

- Live WAN sessions (2026-08-22): governor often holds **1280×720 @ 15 / ~2500 kbps** with 3 viewers. Drop% in the teens froze WebCodecs when RTP had been cut.
- `webrtc-sctp` 0.17.2 dropped whole packets on Chrome FORWARD-TSN until the vendored slice-to-length fix. Age echo is JSON on the **pad** DC (already string-tolerant). Do not invent a new SCTP message type on `video_dc`.
- Host `push_h264` already times out at `PUSH_BUDGET` (50 ms) / `KEYFRAME_PUSH_BUDGET` (1 s). Wake-on-input must not `await` unbounded on that loop.
- Pre-encoded path already ticks at **2 ms** (`is_preencoded()`). Average host wait for a frame that just arrived is ~1 ms, not ~8.3 ms. Do not "oversample the host metronome" again.
- `win_capture.rs` already logs `arrived` / `sent` / `dropped (queue full)` every 5 s. That number never reaches `host_stats` or the browser overlay.
- Encode on the GPU path is **~0 ms** in `host_stats`. Capture-stage 4–5 ms in the 2026-08-15 `OPTIMIZATION_PLAN` is the hop + wait, not MF.
- `CODECAPI_AVLowLatencyMode` is already `1` in `mf_encoder.rs`.
- Pad path is ~250 Hz `CLPD`. Felt lag is input→visible, not pad-Hz by itself. Pad-Hz must not regress (CPU stolen by wake-on-input).
- One-way transit on a good internet path is still ~14 ms. This plan will not beat light.

## File Structure

**Created**

- `crates/proto/src/age.rs` — `AgeStamp` (u64 µs, host monotonic) helpers; `age_ms(now, stamp)`.
- `web/src/ageEcho.ts` — parse CLVD stamp, compute `client_hold_ms`, send pad JSON echo.
- `crates/host/src/age_stats.rs` — ring of last N echo ages; `p50` / `p95` for `HostStats`.

**Modified**

- `crates/proto/src/video_frame.rs` — `VIDEO_VERSION = 3`; header +8 bytes `stamp_us: u64` LE after `frag_count`. `VIDEO_HEADER_LEN = 26`. v2 fragments still decode (no stamp → `stamp_us = 0`).
- `web/src/clvd.ts` — same layout; v2 and v3 both assemble.
- `crates/proto/src/pad_frame.rs` — `PadFeedback::AgeEcho { seq, stamp_us, recv_ms, paint_ms }` is **host←player**; today inbound strings are ignored. Decode this in `setup_pad_channel` without treating it as a `PadFrame`.
- `crates/proto/src/signal.rs` + `web/src/proto.ts` — `HostStats` gains `age_p50_ms`, `age_p95_ms`, `capture_arrived_fps`, `capture_sent_fps`, `capture_queue_dropped` (all optional / default 0 so old tabs keep parsing).
- `crates/host/src/webrtc_peer.rs` — on binary pad: set `expedite` AtomicBool; on AgeEcho JSON: push into `age_stats`.
- `crates/host/src/main.rs` — if `expedite`, run the capture/push branch immediately (do not wait for the 2 ms tick); clear the flag. Include age + capture counters in `host_stats_message`.
- `crates/capture-bridge/src/lib.rs` — reverse byte `EXPEDITE: u8 = b'X'` next to `REQUEST_IDR = b'I'`.
- `crates/capture-bridge/src/bin/win_capture.rs` — on `X`, submit the next encoded AU immediately (skip `next_submit` wait **once**). Log WGC inter-arrival histogram every 5 s (count, mean, p95, max).
- `crates/host/src/capture/hyperv_bridge.rs` — `write_expedite()`; plumb arrived/sent from a tiny `CLAC` (capture accounting) control message **or** parse the existing 5 s log is **not** acceptable — send counters on the socket (see Task 2).
- `web/src/webCodecsCanvas.ts` / `web/src/player.ts` — on assembled AU paint, call `echoAge(...)`.
- `web/src/DebugDrawer.tsx` — show `age p50/p95` next to jitter buffer.

**Not created**

- A second governor. A shared-memory ring. A new video codec. A chroma negotiator.

---

## Risk register and mitigations

Every row is a **rejected design** or a **required guard**. If an implementer "simplifies" past a mitigation, they are off-plan.

| ID | Risk | Why it is real here | Mitigation | How we know it worked |
|----|------|---------------------|------------|------------------------|
| R1 | Cutting RTP again "because dual-send is jitter" | 2026-08-22 freeze after `promoteWebcodecs` | `path_flags` unchanged. Review gate: `PATH_WEBCODECS => (true, true)` | Existing `webcodecs_path_keeps_rtp` (or equivalent) still PASS |
| R2 | CLVD v3 old-tab black picture | Friends sit on cached `web/dist` | v2 fragments still decode; missing stamp is 0 (age ignored). Do **not** reject v2 | Unit: v2 fixture still assembles; v3 fixture round-trips stamp |
| R3 | Age echo on `video_dc` competes with CLVD / FEC | Same channel that sheds | Echo is JSON on **pad** DC only (`msg.is_string` path already exists) | `rg AgeEcho video` empty; pad tests still apply `PadFrame` |
| R4 | Wake-on-input forces IDR / blows GOP | IDR is the expensive frame | Expedite = "encode/send the **next** AU now", never `request_keyframe`, never `force_idr` | Unit: expedite path does not set `keyframe_wanted` |
| R5 | Expedite spins the capture/`select` loop | Pad is ~250 Hz; a wake per report would be a 250 Hz encode | Coalesce: one outstanding `expedite` flag; win-capture honors `X` at most once per target frame interval | Unit: 10 pad frames in 4 ms → one `write_expedite`; pad apply still happens for all 10 |
| R6 | `write_expedite` / age stats block pad inject | Stuck walking is worse than 8 ms of video wait | Expedite is `store(true)` on the pad callback; socket write happens on the video loop. Age echo must not `apply()` a pad | Test: AgeEcho JSON does not call `VirtualPad::apply` |
| R7 | Header length walk (SCTP class of bug) | v3 +8 bytes; assembler uses `VIDEO_HEADER_LEN` | Slice to version: v2 → 18, v3 → 26. Never `buf[offset..]` remainder as payload start | Rust + TS: v3 fragment payload starts at byte 26; FEC recover still uses `VIDEO_MAX_FRAGMENT_PAYLOAD` |
| R8 | Governor treats expedite sheds as congestion | `on_window(shed, sent)` already yo-yoed | Expedite timeout uses the same `PUSH_BUDGET` as a normal frame and counts as a normal shed if it times out — **do not** add a second shed type, and **do not** increment extra | Existing governor tests unchanged |
| R9 | Phase-lock without measuring spread | 2026-08-06 plan: if spread ≈ 16.7 ms, phase estimate is noise | Task 4 logs histogram **before** any `next_submit` offset. Task 5 is skipped if p95(inter-arrival) > 8 ms at 60 Hz | Gate in the Task 5 commit message: paste the histogram |
| R10 | Replacing Hyper-V with shm "because the audit said so" | vsock is already the handoff; shm is a new primitive | Task 6 measures hop with the age stamp (win-capture write Instant vs host read Instant, both logged). If hop p95 < 2 ms, **stop** | Decision recorded in the Task 6 commit; no shm code if under gate |
| R11 | Host restart wedges win-capture / "waiting for host" | Live incident 2026-08-22 | `HyperVBridge::connect` still returns without `read_one()`. New reverse bytes are write-only after connect | Host restart still reaches `capturing` then offers |
| R12 | Killing `ds-vhid` / PCSX2 "to test lag" | Product rule | Test plan forbids it. Use the already-running game + one friend hard-refresh | Checklist item |
| R13 | Chroma / 1080p60-444 sneak into this PR | PR #42 temptation | Out of scope. Separate branch if ever | `rg 444` / `Hi444` empty in the implementation commits |
| R14 | TURN-TCP still broken; extra signaling for age | Existing `transport=tcp` failure | Age uses the existing pad DC. No new ICE / m-line | ICE connect time unchanged |

**Hard no's (if you are about to do one of these, stop):**

- Cut RTP after first WebCodecs paint to "save the dual-send tax."
- Force an IDR on every pad press.
- Put age / expedite bytes on `video_dc`.
- Feed wake-on-input into `link_gov`.
- Phase-lock or shm without the Task 4 / Task 6 numbers.
- Software x264 `zerolatency` + `high444` as a lag experiment.
- Delay video to match future audio (see browser-audio plan).
- `prebind_all()` or kill `couchlink-ds-vhid` to "simplify testing."

---

## Regression testing (must stay green before a friend joins)

These protect 2026-08-22 video/pad/session behavior. Run in CI / `cargo test` + `npm test` **before** anyone hard-refreshes the live invite.

### R.1 CLVD v2/v3 wire

- [ ] **Write the failing test** in `crates/proto/src/video_frame.rs`:

```rust
#[test]
fn v2_fragment_still_decodes_without_a_stamp() {
    // encode a v2-shaped 18-byte header + payload by hand (VIDEO_VERSION = 2)
    // decode must succeed; stamp_us == 0
}

#[test]
fn v3_round_trip_preserves_stamp_and_does_not_eat_payload() {
    let au = VideoAccessUnit { seq: 7, width: 1280, height: 720, keyframe: true, annex_b: vec![0,0,0,1,0x65], stamp_us: 1_234_567 };
    let frags = au.encode_fragments();
    let back = assemble(&frags).unwrap();
    assert_eq!(back.stamp_us, 1_234_567);
    assert_eq!(back.annex_b, au.annex_b);
}

#[test]
fn v3_header_is_26_bytes_and_fec_parity_still_recovers_one_loss() {
    // existing single-loss FEC test, but with stamp_us != 0 on every fragment
}
```

- [ ] Run: `cargo test -p couchlink-proto v3_round_trip v2_fragment -- --nocapture`  
      Expected after implementation: PASS. Before: FAIL (no `stamp_us`).

- [ ] Mirror in `web/src/clvd.ts` (or `clvd.test.ts`): v2 fixture + v3 fixture + one-loss FEC with stamp.

### R.2 Age echo is not a pad

- [ ] In `crates/host/src/webrtc_peer.rs` tests (or a new `age_echo` module test):

```rust
#[test]
fn age_echo_json_does_not_apply_to_virtual_pad() {
    // feed {"type":"age_echo","seq":1,"stamp_us":9,"recv_ms":1.0,"paint_ms":2.0}
    // VirtualPad buttons stay neutral; age_stats records one sample
}
```

- [ ] Existing: `cargo test -p couchlink-host apply_pad_bytes` (and DualSense/Xbox sim tests) PASS.

### R.3 Expedite coalesce + no IDR

```rust
#[test]
fn ten_pad_frames_set_expedite_once() {
    // 10 PadFrame::neutral() (or one button) into the pad callback
    // write_expedite call count == 1 (or AtomicBool true once; video loop clears it)
    // keyframe_wanted == false
}

#[test]
fn expedite_does_not_call_link_gov() {
    // fire expedite; link_gov.current() unchanged
}
```

- [ ] Re-run:  
      `cargo test -p couchlink-host two_clean_windows_do_not_climb`  
      `cargo test -p couchlink-host webcodecs_path_keeps_rtp`  
      `cargo test -p webrtc-sctp --manifest-path vendor/webrtc-sctp-0.17.2/Cargo.toml test_forward_tsn_bundled_with_sack_parses`

### R.4 Capture accounting message

```rust
#[test]
fn clac_then_clf2_round_trip_without_eating_the_next_frame() {
    // write CLAC (arrived, sent, dropped u64 LE each) + CLF2 into a Cursor
    // read accounting, then video; a 1-byte-short CLAC must not consume the CLF2
}
```

- [ ] `cargo test -p couchlink-capture-bridge clac_then_clf2 -- --nocapture`

### R.5 Web player

- [ ] `ageEcho` does not call `promoteWebcodecs` / `preferRtpPresent`.
- [ ] `npm test -- --run` for `clvd`, `webCodecsCanvas`, `latencyStats`, new `ageEcho.test.ts`.
- [ ] `path_flags` TypeScript-side present path still reports `warmup` until first paint, then `webcodecs`, RTP canvas stays decoding.

### R.6 Live video baseline (manual, before enabling wake-on-input)

Record a 60 s window on the **current** host (age code may already be logging zeros):

| Metric | Where | Floor |
|--------|--------|-------|
| streaming fps | `[couchlink-host] streaming` | tonight's ~15 fps WAN floor ±1 |
| drop% | same line | no new sustained >8% from this work |
| `chunk too short` | host log | stay 0 |
| pad Hz | browser overlay | ≥100 Hz |
| ICE `connectionState` | host log | `connected` |
| `path_flags` | code | WebCodecs still gets RTP |

Save the log snippet. Task 3 is not allowed to worsen drop% or pad Hz by more than measurement noise (~5%).

---

## Integration testing (after unit green; live stack)

Do **not** kill PCSX2, `ds-vhid`, or win-capture's window needle. Restart **host and/or signaling only** if the new binary requires it. Friends hard-refresh the same invite. Rebuild `web/dist` (`cd web && npm run build`) so signaling serves the v3 decoder.

### I.1 Age is a number, not a vibe

1. One friend, hard-refresh, play 60 s.
2. Host log and overlay show `age_p50_ms` / `age_p95_ms` that move (not stuck at 0).
3. Press a face button: `age` samples continue (echo still on pad DC).
4. Fail: age stays 0 with v3 fragments in flight — stamp is not reaching paint.
5. Fail: pads dead — AgeEcho stole the binary path (R2).

### I.2 Three viewers (the real shape)

1. Three browsers, mix of KBM and pads.
2. 3 minutes of actual play.
3. Pass:
   - all three show age numbers
   - no WebCodecs freeze that does not recover via the already-shipped RTP rescue
   - zero `chunk too short`
   - no "Waiting for host offer"
   - P2–P4 input still registers in PCSX2
   - pad Hz ≥100 on each client
4. Fail and **stop** if video drop% jumps >8 points vs R.6 — revert Task 3, do not "fix" by cutting RTP.

### I.3 Wake-on-input A/B (same invite, same friend)

1. `COUCHLINK_WAKE_ON_INPUT=0`, 60 s, record age p50/p95 and a button→visible phone-camera clip if you have one.
2. Restart **host only** with wake on (default). Same friend, hard-refresh.
3. Predict: age p50 drops by roughly one encode interval (up to ~16 ms at 60 Hz, less at the 15 fps governor floor — **expect a smaller win at 15 fps**; do not declare failure because WAN is fps-bound).
4. Refuted if: p50 unchanged **and** win-capture log never shows an expedite submit. Then the flag is not reaching Windows — fix the reverse byte, don't tune the governor.
5. Fail if: drop% or `chunk too short` appears, or pad apply stutters.

### I.4 Host restart

1. Kill **only** `couchlink-host`, start the new binary with the same `--session-id/--pin/--turn-* /--windows-capture hyperv:9877`.
2. Pass: `registered as Host` then `capturing` without blocking on first video frame; seated players get offers; age resumes after refresh.
3. Fail: sitting on `Hyper-V capture socket connected` with no `capturing` — that is R11.

### I.5 WGC histogram (Task 4, no behavior change)

1. 60 s of gameplay, read win-capture log line `wgc interarrival p50=… p95=…`.
2. If p95 ≤ 8 ms at a 60 Hz capture beat, Task 5 is allowed.
3. If p95 approaches 16.7 ms, **write "phase-lock abandoned" in the Task 4 commit** and skip Task 5.

### I.6 Hop (Task 6, no shm unless gate fails)

1. With age stamps in place, log `hop_ms = host_read - win_write` only if both clocks are the **same** Instant domain — they are not (Windows vs WSL).
2. Honest hop: host sends a `CLHP` ping (u64 host Instant) on the reverse socket; win-capture echoes it on the next `CLAC`/`CLHP` reply; host computes RTT/2.
3. If hop p95 < 2 ms, do not start a shared-memory project.

### I.7 Off-switch

1. `COUCHLINK_WAKE_ON_INPUT=0`, three viewers.
2. Pass: identical video/pad behavior to R.6; age still reports (instrument stays on).

---

### Task 1: CLVD v3 stamp + pad AgeEcho + host p50/p95

**Files:**
- Create: `crates/proto/src/age.rs`, `crates/host/src/age_stats.rs`, `web/src/ageEcho.ts`, `web/src/ageEcho.test.ts`
- Modify: `crates/proto/src/video_frame.rs`, `crates/proto/src/pad_frame.rs`, `crates/proto/src/signal.rs`, `crates/proto/src/lib.rs`, `web/src/clvd.ts`, `web/src/proto.ts`, `web/src/player.ts`, `web/src/webCodecsCanvas.ts`, `web/src/DebugDrawer.tsx`, `crates/host/src/webrtc_peer.rs`, `crates/host/src/main.rs`
- Test: proto v2/v3 + FEC; `age_echo_json_does_not_apply_to_virtual_pad`

**Interfaces:**
- Produces:
  - `VideoAccessUnit.stamp_us: u64` (0 = unknown)
  - `VideoFragment.stamp_us: u64` — same on every fragment of an AU, including FEC parity
  - `PadFeedback::AgeEcho { seq: u32, stamp_us: u64, recv_ms: f64, paint_ms: f64 }`
  - `AgeStats::record(age_ms: f64)` / `AgeStats::percentiles() -> (f64, f64)`
  - `HostStats.age_p50_ms: f64`, `age_p95_ms: f64`
- Host stamps `stamp_us` from `std::time::Instant` converted to µs since host start (`age::origin()`), **at Hyper-V read**, not at encode. That is glass-to-glass minus the Windows-side wait (Task 6 covers the hop). Clock offset cancels because the client echoes the same `stamp_us` and the host computes `now - stamp` when the echo arrives. Also log client `paint_ms - recv_ms` as `client_hold_ms` in the browser overlay (no clock sync).

- [ ] **Step 1:** Write R.1 and R.2 tests. Run them. Confirm FAIL.
- [ ] **Step 2:** Bump encode to v3; decode v2 + v3. Keep `VIDEO_MAX_FRAGMENT_PAYLOAD = 14_000`.
- [ ] **Step 3:** Browser decode + `echoAge` on first paint of an AU (once per `seq`, not per fragment).
- [ ] **Step 4:** Host pad-string path: if `type == age_echo`, record `age_ms`; do not `PadFrame::decode`.
- [ ] **Step 5:** Tests PASS. `npm test`. `npm run build`.
- [ ] **Step 6:** Commit `feat(proto): CLVD v3 capture stamp and pad age echo`

### Task 2: Capture arrived/sent on the overlay

**Files:**
- Modify: `crates/capture-bridge/src/lib.rs`, `crates/capture-bridge/src/bin/win_capture.rs`, `crates/host/src/capture/hyperv_bridge.rs`, `crates/host/src/capture/bridge.rs`, `crates/proto/src/signal.rs`, `crates/host/src/main.rs`, `web/src/proto.ts`, `web/src/DebugDrawer.tsx`
- Test: R.4 `clac_then_clf2_round_trip_without_eating_the_next_frame`

**Interfaces:**
- Produces: `pub const ACCOUNT_MAGIC: &[u8; 4] = b"CLAC";`  
  `pub struct CaptureAccount { pub arrived: u64, pub sent: u64, pub dropped: u64 }`  
  `write_account` / `read_account` — magic + `u32` LE length + body; reader slices **to length**.
- win-capture already has the counters. Write a `CLAC` about every 1 s (not only the 5 s log).
- Host `read` already branches on magic in spirit (`FRAME_MAGIC`); add `CLAC` → pending account, leave `CLF2` as video.
- `HyperVBridge::connect` still must not `read_one()`.

- [ ] **Step 1:** Write R.4. Confirm FAIL.
- [ ] **Step 2:** Implement `CLAC`. Host copies into `HostStats.capture_*`.
- [ ] **Step 3:** Overlay shows arrived vs sent. A gap here is the OPTIMIZATION_PLAN leak; do not retune the governor because of it.
- [ ] **Step 4:** Commit `feat(capture): CLAC arrived/sent counters on the Hyper-V socket`

### Task 3: Wake-on-input

**Files:**
- Modify: `crates/capture-bridge/src/lib.rs`, `crates/capture-bridge/src/bin/win_capture.rs`, `crates/host/src/capture/hyperv_bridge.rs`, `crates/host/src/webrtc_peer.rs`, `crates/host/src/main.rs`
- Test: R.3

**Interfaces:**
- Produces: `pub const EXPEDITE: u8 = b'X';`  
  `WebRtcHost::take_expedite() -> bool`  
  `HyperVBridge::write_expedite() -> Result<()>` (non-blocking / ignore would-block)
- Consumes: pad `on_message` binary success → `expedite.store(true)`.
- Host video `select!`: if `take_expedite()`, write `X`, then try one capture drain + `push_h264` immediately (same `PUSH_BUDGET`).
- win-capture: on `X`, set `next_submit = Instant::now()` once. Do not change the commanded fps/bitrate. Do not emit an extra IDR.
- `COUCHLINK_WAKE_ON_INPUT=0` skips the store and the `X`.

- [ ] **Step 1:** Write R.3 tests. Confirm FAIL.
- [ ] **Step 2:** Atomic flag + coalesce.
- [ ] **Step 3:** Reverse `X` + skip one `next_submit` wait.
- [ ] **Step 4:** Tests PASS. Existing path_flags / governor / SCTP tests PASS.
- [ ] **Step 5:** Commit `feat(input): expedite the next capture frame after a pad report`

### Task 4: WGC inter-arrival histogram (measure only)

**Files:**
- Modify: `crates/capture-bridge/src/bin/win_capture.rs`

**Interfaces:**
- On `on_frame_arrived`, record `now - last_wgc` (even when you drop for `frame_dur`).
- Every 5 s (same window as arrived/sent): log `wgc interarrival n= p50= p95= max= ms`.

- [ ] **Step 1:** Add the histogram (fixed 64-bin or a small sorted vec capped at 512).
- [ ] **Step 2:** Run I.5. Paste the line into the commit message.
- [ ] **Step 3:** Commit `chore(capture): log WGC inter-arrival p50/p95`

### Task 5: Phase-lock (gated)

**Files:**
- Modify: `crates/capture-bridge/src/bin/win_capture.rs` only if I.5 passed the 8 ms gate.

**Interfaces:**
- Estimate phase from the Task 4 window. Delay `next_submit` to fire just after the clustered WGC arrivals, still at the commanded fps.
- `COUCHLINK_PHASE_LOCK=0` disables.
- **Predict:** ~½ of the residual WGC wait off age p50 at unchanged CPU.
- **Refuted if:** age p50 unchanged or p95 worse (you queued behind composition). Revert this task.

- [ ] **Step 1:** Implement behind the env flag, default **off** until I.3 is re-run with it on.
- [ ] **Step 2:** I.3-style A/B. If refuted, revert and commit `revert: phase-lock (spread too wide)`.
- [ ] **Step 3:** If confirmed, commit `feat(capture): phase-lock encode submit to WGC arrivals`

### Task 6: Hop ping (gated shm)

**Files:**
- Modify: `crates/capture-bridge/src/lib.rs`, `win_capture.rs`, `hyperv_bridge.rs`, `age_stats` / `HostStats` (`hop_p50_ms`, `hop_p95_ms`)

**Interfaces:**
- `pub const HOP_MAGIC: &[u8; 4] = b"CLHP";` host → win: magic + u64 `host_us`. win → host: same magic + echoed `host_us`.
- Host every 1 s writes one ping on the reverse socket; on echo, `hop_ms = (now_us - host_us) / 2000` (RTT/2).
- If hop p95 < 2 ms: **stop**. Record in commit. No ring buffer.
- If hop p95 ≥ 2 ms: **new plan**, not this file. Do not start shm in the same PR as wake-on-input.

- [ ] **Step 1:** CLHP round-trip unit test (same "don't eat CLF2" shape as CLAC).
- [ ] **Step 2:** Live I.6. Commit the number: `chore(capture): Hyper-V hop p50/p95 = …`

### Task 7: Run R.* then I.*

- [ ] **Step 1:** Full regression list R.1–R.5.
- [ ] **Step 2:** Record R.6 baseline if not already saved.
- [ ] **Step 3:** I.1 → I.4, then I.5. Stop at first fail; do not stack "fixes."
- [ ] **Step 4:** Task 5 / Task 6 only after their gates. I.7 last.
- [ ] **Step 5:** Implementation PRs merge only if I.2 and I.3 pass. If I.3 fails drop%, ship `COUCHLINK_WAKE_ON_INPUT=0` as default and keep the age instrument.

---

## Execution notes

- This commit is **plan only**. Implementation happens on follow-up branches (`feat/clvd-age-stamp`, `feat/wake-on-input`, …), not mixed with roster/KBM viz.
- First live enable: one friend, then three. Same invite, hard-refresh for JS (`web/dist`).
- Rollback: `COUCHLINK_WAKE_ON_INPUT=0`. Age stamps can stay — they are the instrument.
- Ethernet still beats Wi-Fi for variance (`LATENCY.md`). That is not a code task.
- Nothing here beats ~14 ms of one-way transit. If someone wants single-digit glass-to-glass, that is proximity.

Plan complete. Implementation is **not** in this commit.
