# Browser game-audio Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Friends hear PCSX2 (or whatever the host is playing) in the browser, without making the picture freeze, shed, or climb the governor again.

**Architecture:** Capture loopback audio on Windows next to H.264, encode Opus there, carry tiny frames over the existing Hyper-V capture socket as a new magic, and fan them out on a **separate WebRTC RTP audio track**. Never put audio on the CLVD / pad DataChannels. The browser attaches an `<audio>` element from `ontrack` (`kind === "audio"`) and leaves the WebCodecs / RTP-video path untouched. A dead or silent audio path degrades to video-only; it must not take the session down.

**Tech Stack:** WASAPI loopback (Windows, in `couchlink-win-capture`), Opus 48 kHz stereo ~48 kbps / 20 ms, Hyper-V vsock (same `CLF2` sibling framing), webrtc-rs `TrackLocalStaticSample` + `MIME_TYPE_OPUS` (already registered by `register_default_codecs()`), Chrome `RTCPeerConnection.ontrack`.

## Status

| Task | State |
|------|-------|
| 0. This plan (risk + test gates) | **This commit** — implementation not started |
| 1. Capture-socket audio frame type | Not started |
| 2. WASAPI loopback + Opus in win-capture | Not started |
| 3. Host Opus RTP track + fan-out | Not started |
| 4. Browser `<audio>` attach | Not started |
| 5. Regression suite (video/pad/governor) | Not started |
| 6. Integration suite (1- and 3-viewer audio) | Not started |

## Global Constraints

- **Do not close PCSX2 / the game** to implement or verify. Host / win-capture / signaling restarts are allowed; `couchlink-ds-vhid` is not killed mid-session.
- Audio is **one encode, N RTP copies** — same rule as video. Never encode Opus per peer.
- Audio **must not** share `video_dc` or `pad_dc`. The 2026-08-22 freezes were CLVD sheds + WebCodecs-only present. Putting audio on that channel is a rejected design, not a fallback.
- `push_h264` / `PUSH_BUDGET` / the link governor stay video-only. Lost Opus packets are clicks, not a reason to step 720p down.
- `register_default_codecs()` already registers Opus in `crates/host/src/webrtc_peer.rs`. Do not add a second codec registry.
- RTP video **stays live** after WebCodecs first paint (`path_flags` keeps RTP on). Audio has no "promote" and is never cut.
- Failure mode is always **video-only**, never "no offer" / wedged `HyperVBridge::connect` / dead pad.
- Off by `COUCHLINK_AUDIO=0` (or win-capture `--no-audio`) so a bad machine can ship picture without a rebuild.
- Do not implement this plan in the same PR as unrelated UI (roster colors, KBM viz). Audio is its own commit series.

## Measured facts (2026-08-22)

Do not re-derive these to justify the design. They are why the mitigations exist.

- Video tonight sat around **1280×720 @ ~15 fps / 2500 kbps** after the governor floor, with **3 WAN viewers**.
- CLVD sheds in the teens-to-thirties percent **froze WebCodecs** when RTP had been cut. RTP is now kept live; do not re-introduce a second "cut the safety path" for audio.
- `webrtc-sctp` 0.17.2 dropped whole packets on Chrome FORWARD-TSN until we vendored a slice-to-chunk-length fix. Audio RTP does not go through that parser. Do not "also send Opus on SCTP so it's low-latency."
- Host `push_h264` already times out and sheds; the capture loop is the one async thread. Any audio `write_sample` that can block that loop is the same class of bug as `HyperVBridge::connect` waiting on `read_one()`.
- Opus stereo at 48 kbps is ~**2%** of a 2500 kbps video send, even ×3 peers (~144 kbps vs ~7.5 Mbps). CPU for WASAPI + Opus is negligible next to GPU H.264. **Bitrate/CPU are not the regression.** Congestion coupling and Chrome audio jitter are.
- Chrome's video WebCodecs path can paint with almost no jitter buffer; **RTP audio typically sits 50–150 ms behind**. That is A/V skew, not fps loss. Do not delay video to match audio.

## File Structure

**Created**

- `crates/capture-bridge/src/audio.rs` — `AudioFrame` (seq, sample_rate, channels, opus payload), `AUDIO_MAGIC = b"CLA1"`, `write_audio_frame` / `read_audio_frame` on the same socket as `CLF2`.
- `crates/capture-bridge/src/wasapi_loopback.rs` (Windows-only) — capture default render device, feed Opus encoder.
- `crates/host/src/audio.rs` — drain `CLA1` from the capture bridge, `write_sample` onto each peer's Opus track with a short timeout; never `await` unbounded on the video loop.
- `web/src/audio.ts` — `attachAudioTrack(track)` + `detachAudio()`; jitter-buffer pin if the browser exposes it.
- `crates/host/src/audio_gov.rs` is **not** created. Video governor stays the only governor.

**Modified**

- `crates/capture-bridge/src/lib.rs` — export audio frame codec; do not change `FRAME_MAGIC` / `CLF2`.
- `crates/capture-bridge/src/bin/win_capture.rs` — spawn loopback+Opus thread; multiplex `CLA1` writes with H.264 writes (one socket, two magics).
- `crates/host/src/capture/hyperv_bridge.rs` and `bridge.rs` — `read_frame` already distinguished by magic; add `CLA1` → pending audio, leave `CLF2` as video.
- `crates/host/src/webrtc_peer.rs` — `audio: Arc<TrackLocalStaticSample>` (`MIME_TYPE_OPUS`), `add_track` next to video, `push_opus(payload, Duration::from_millis(20))`.
- `crates/host/src/main.rs` — after each capture drain, if an Opus frame is pending, `push_opus` to all slots with its own 10 ms budget (not `PUSH_BUDGET`).
- `web/src/player.ts` — `ontrack`: if `track.kind === "audio"` call `attachAudioTrack`; do not set `gotVideoTrack` from audio.
- `web/src/App.tsx` — hidden `<audio autoplay playsInline>` ref; no UI required for v1.

---

## Risk register and mitigations

Every row is a **rejected design** or a **required guard**. If an implementer "simplifies" past a mitigation, they are off-plan.

| ID | Risk | Why it is real here | Mitigation | How we know it worked |
|----|------|---------------------|------------|------------------------|
| R1 | Audio on CLVD / SCTP competes with video and re-freezes WebCodecs | 2026-08-22 sheds of 15–33% froze the last picture when RTP was cut | **RTP audio track only.** No Opus on `video_dc`. Code review gate: `rg opus video_dc` is empty | Regression test: with audio on, CLVD drop% over 60s is within 3 points of the no-audio baseline on the same link |
| R2 | `write_sample` audio blocks the capture/`select` loop | `read_one()` on connect already caused "Waiting for host offer" | `tokio::time::timeout(10ms, push_opus)`; on timeout drop **that audio frame only**, do not `request_keyframe`, do not increment video `dropped_frames` | Unit test: a hung audio sender cannot delay a subsequent `push_h264` beyond the timeout |
| R3 | Governor treats audio loss as video shed and yo-yos bitrate | Governor `on_window(shed, sent)` already yo-yoed at 2% | Audio timeouts **do not** call `link_gov.on_window` | Existing `two_clean_windows_do_not_climb` still passes; new test `opus_timeout_does_not_step_governor` |
| R4 | Chrome audio jitter buffer adds 100ms+ and someone "fixes" it by delaying video | WebCodecs present is the low-latency path | Pin audio `jitterBufferTarget = 0` / `playoutDelayHint = 0` if present. **Never** add delay to WebCodecs or CLVD to A/V-sync | Integration: video `presentFps` unchanged vs baseline; log audio delay separately |
| R5 | Loopback captures the wrong device (mic, or a silent render) | WASAPI default can be HDMI/TV with no speakers, or a comms headset | Capture **loopback of the default render device**; log the device name at start; `COUCHLINK_AUDIO_DEVICE=` override; if 2s of digital silence, log once and keep sending (video stays up) | Integration: play a 440 Hz tone in PCSX2/Windows, browser `<audio>` `AnalyserNode` sees energy > threshold |
| R6 | Multiplex bug corrupts `CLF2` (video magic / length) | Same vsock as video; a wrong length walk is the SCTP "chunk too short" class of bug | `CLA1` has its own 4-byte magic + `u32` LE length; reader slices **to length**, never `buf[offset..]` remainder | Capture-bridge unit tests: `CLA1` bundled after `CLF2` in one read stream parses both; truncated `CLA1` does not consume the next `CLF2` |
| R7 | 3-viewer fan-out of audio + video+CLVD saturates the host send path | We already shed on 3 WAN peers at 2500 kbps | Audio is ~48 kbps × N. If video is already shedding >8%, **do not** raise video bitrate to "make room for audio." Optional: drop audio before video (opposite of telephony) — game picture wins | Integration: 3 viewers, 60s; video drop% not worse than baseline+3; no new `chunk too short` |
| R8 | Autoplay blocked, user thinks "audio is broken" and we restart capture | Browsers need a gesture | First click/tap on the stage calls `audioEl.play()`; one-line hint "click for sound" until `playing` | Manual: hard-refresh, click stream, sound starts; no host restart |
| R9 | Offer/SDP grows an audio m-line and old tabs fail ICE | Friends may sit on old JS | Audio m-line is standard; old Chrome still answers. If `add_track` audio fails, continue video-only | Host log: `audio track added` or `audio disabled, video only` — never fail `create_offer` |
| R10 | Host restart while implementing wedges win-capture / "waiting for host" | Live incident 2026-08-22 | `HyperVBridge::connect` must still return without `read_one()`. Audio thread starts **after** the video socket is up | Regression: host restart with running win-capture still reaches `capturing` and sends offers |
| R11 | Killing `ds-vhid` or PCSX2 "to test audio" | Product rule | Test plan forbids it. Audio tests use a tone file or the already-running game | Checklist item, not a test assertion |
| R12 | TURN-TCP still broken; extra track makes gather worse | Existing `Unable to handle URL … transport=tcp` | Do not add audio-specific ICE. Audio uses the same PC. If ICE is UDP-only today, it stays UDP-only | ICE connect time for 3 peers within 2s of baseline |

**Hard no's (if you are about to do one of these, stop):**

- Opus or PCM on the video DataChannel "just for this session."
- Cutting the audio RTP track after first WebCodecs paint to "save bandwidth."
- Feeding audio sheds into `link_gov`.
- Sleeping the video cadence on audio backpressure.
- Capturing from WSL Pulse/`wslg` instead of Windows loopback.
- Requiring friends to join before launching PCSX2 so audio devices enumerate.

---

## Regression testing (must stay green with audio off **and** on)

These protect tonight's video/pad/session behavior. They run in CI / `cargo test` + `npm test` **before** anyone joins a live session with the new bits.

### R.1 Capture framing

- [ ] **Write the failing test** in `crates/capture-bridge/src/audio.rs` (or `lib.rs` tests):

```rust
#[test]
fn clf2_then_cla1_round_trip_without_eating_the_next_frame() {
    // write CLF2 (video) + CLA1 (opus) + CLF2 into a Cursor
    // read three frames: video, audio, video
    // a 1-byte-short CLA1 must return err and leave the following CLF2 intact
}
```

- [ ] Run: `cargo test -p couchlink-capture-bridge clf2_then_cla1 -- --nocapture`  
      Expected after implementation: PASS. Before: FAIL (type missing).

### R.2 Host push isolation

- [ ] Test `push_opus` timeout does not increment video shed:

```rust
#[test]
fn opus_timeout_is_not_a_video_shed() {
    // mock / channel that never completes
    // push_opus times out
    // link_gov.current() unchanged; video dropped_frames unchanged
}
```

- [ ] Re-run existing:  
      `cargo test -p couchlink-host two_clean_windows_do_not_climb`  
      `cargo test -p couchlink-host webcodecs_path_keeps_rtp`  
      Expected: PASS with audio code present.

### R.3 SCTP / ICE / signaling (no audio in these paths)

- [ ] `cargo test -p webrtc-sctp --manifest-path vendor/webrtc-sctp-0.17.2/Cargo.toml test_forward_tsn_bundled_with_sack_parses`  
      Expected: PASS (audio must not touch this crate).
- [ ] `cargo test -p couchlink-signaling replay_for_host reconnecting_host`  
      Expected: PASS.

### R.4 Web player

- [ ] `ontrack` audio does not flip present path to RTP-only or stop WebCodecs:

```ts
it("audio track does not call promoteWebcodecs or preferRtpPresent", () => {
  // stub PC ontrack with kind audio
  // present path stays warmup/webcodecs as before
});
```

- [ ] `npm test -- --run src/webCodecsCanvas.test.ts src/player.ts` (and the new `audio.test.ts`)  
      Expected: PASS.

### R.5 Off switch

- [ ] With `COUCHLINK_AUDIO=0`, offer SDP has **no** `m=audio` (or has it inactive). Video offer unchanged.  
      Test: parse host-generated SDP fixture in a unit test once `create_offer` is reachable, or snapshot the transceiver count (`audio_transceivers == 0`).

### R.6 Live video baseline (manual, before enabling audio in prod)

Record a 60s window on the **current** host (audio not shipped):

| Metric | Where | Baseline to keep |
|--------|--------|------------------|
| streaming fps | `[couchlink-host] streaming` | ~same as tonight's 15 fps floor if WAN |
| drop% | same line | no new sustained >8% from audio later |
| `chunk too short` | host log | stay 0 after the SCTP vendor patch |
| pad Hz | browser overlay | ≥100 Hz |
| ICE `connectionState` | host log | stays `connected` |

Save the log snippet as the integration comparison, not a vibe.

---

## Integration testing (after unit green; live stack)

Do **not** kill PCSX2, `ds-vhid`, or win-capture's window needle. Restart **host and/or signaling only** if the new binary requires it. Friends hard-refresh the same invite.

### I.1 Single viewer, tone

1. Host: audio on, default render device is a real speaker/headset (not a dead HDMI).
2. Play a known tone (Windows sound tester or in-game menu music).
3. One friend: hard-refresh, click the stream once.
4. Pass: they hear the tone; host still `streaming` at the pre-audio fps ±1; drop% not up >3 points over 60s; pads still work.

### I.2 Three viewers (the real shape)

1. Three browsers, mix of KBM and pads.
2. 3 minutes of actual play (not a loading screen).
3. Pass:
   - all three hear audio (after click if needed)
   - no WebCodecs freeze that does not recover via the already-shipped RTP rescue
   - zero `chunk too short`
   - no "Waiting for host offer"
   - P2–P4 input still registers in PCSX2
4. Fail and **stop** if video drop% jumps >8 points vs the R.6 baseline — pull audio off (`COUCHLINK_AUDIO=0`), do not "fix" by cutting RTP or shrinking CLVD in the same change.

### I.3 Audio death does not kill video

1. Set `COUCHLINK_AUDIO_DEVICE` to a nonexistent id, or stop the loopback thread.
2. Pass: video and pads continue; host logs `audio silent/disabled`; browser stays on picture.

### I.4 Host restart (audio-enabled binary)

1. Kill **only** `couchlink-host`, start the new binary with the same session/PIN/TURN/hyperv:9877.
2. Pass: `host registered` then `capturing` without blocking on first video frame; seated players get offers (signaling replay); audio returns after refresh or automatically if the PC stayed up.
3. Fail: sitting on `Hyper-V capture socket connected` with no `capturing` — that is R10, not an audio bug to paper over.

### I.5 Autoplay / mute

1. Fresh tab, no click: picture OK, maybe no sound.
2. Click stage: sound starts.
3. Fail: needing a host restart or a second invite for sound.

### I.6 A/V skew (observe, do not "fix" by delaying video)

1. A visible in-game event with a hard sound (menu confirm, attack).
2. Record whether audio feels late vs WebCodecs.
3. Log-only: if >200 ms late, file a follow-up (smaller Opus frame / audio playout hints). **Do not** add a video delay in this plan's PR.

### I.7 Off-switch integration

1. `COUCHLINK_AUDIO=0`, three viewers.
2. Pass: identical video/pad behavior to R.6; no audio track events in `chrome://webrtc-internals`.

---

### Task 1: CLA1 frame on the capture socket

**Files:**
- Create: `crates/capture-bridge/src/audio.rs`
- Modify: `crates/capture-bridge/src/lib.rs`
- Test: `crates/capture-bridge/src/audio.rs` (or `lib.rs` `#[cfg(test)]`)

**Interfaces:**
- Produces: `pub const AUDIO_MAGIC: &[u8; 4] = b"CLA1";`  
  `pub struct AudioAccessUnit { pub seq: u32, pub sample_rate: u32, pub channels: u8, pub opus: Vec<u8> }`  
  `pub fn write_audio_frame(w: &mut impl Write, au: &AudioAccessUnit) -> Result<()>`  
  `pub fn read_audio_frame(r: &mut impl Read) -> Result<AudioAccessUnit>` — reads magic+length first, then exactly `length` bytes.

- [ ] **Step 1:** Write `clf2_then_cla1_round_trip_without_eating_the_next_frame` (see R.1).
- [ ] **Step 2:** Run it; confirm FAIL.
- [ ] **Step 3:** Implement magic + length-prefixed body (seq `u32` LE, sample_rate `u32` LE, channels `u8`, rest = opus).
- [ ] **Step 4:** Test PASS.
- [ ] **Step 5:** Commit `feat(capture): CLA1 Opus frames on the win-capture socket`

### Task 2: WASAPI loopback + Opus (Windows)

**Files:**
- Create: `crates/capture-bridge/src/wasapi_loopback.rs`
- Modify: `crates/capture-bridge/src/bin/win_capture.rs`

**Interfaces:**
- Consumes: `write_audio_frame`
- Produces: a thread that, while the video socket is connected, writes one `AudioAccessUnit` about every 20 ms. `--no-audio` / `COUCHLINK_AUDIO=0` skips the thread.

- [ ] **Step 1:** Log the render device name at start (R5).
- [ ] **Step 2:** Encode 48 kHz stereo Opus ~48 kbps, 20 ms.
- [ ] **Step 3:** If write fails, log and continue video; do not exit `win_capture`.
- [ ] **Step 4:** Commit `feat(win-capture): WASAPI loopback to CLA1`

### Task 3: Host Opus track

**Files:**
- Create: `crates/host/src/audio.rs`
- Modify: `crates/host/src/webrtc_peer.rs`, `crates/host/src/main.rs`, `crates/host/src/capture/hyperv_bridge.rs`

**Interfaces:**
- Produces: `WebRtcHost::push_opus(&self, opus: Vec<u8>, duration: Duration) -> Result<bool>`  
  `true` = shed (timeout); **not** a video shed.  
  `HyperVBridge` / `WindowsBridge` yield `Option<AudioAccessUnit>` alongside video.

- [ ] **Step 1:** `add_track` Opus; `COUCHLINK_AUDIO=0` skips `add_track` (R5/R9).
- [ ] **Step 2:** Drain `CLA1` off the capture socket without blocking connect (R10).
- [ ] **Step 3:** Fan-out with 10 ms timeout (R2, R3).
- [ ] **Step 4:** Tests in R.2 PASS; existing path_flags / governor tests PASS.
- [ ] **Step 5:** Commit `feat(host): RTP Opus track, isolated from CLVD and the governor`

### Task 4: Browser attach

**Files:**
- Create: `web/src/audio.ts`, `web/src/audio.test.ts`
- Modify: `web/src/player.ts`, `web/src/App.tsx`

**Interfaces:**
- Produces: `attachAudioTrack(track: MediaStreamTrack, el: HTMLAudioElement): void`  
  sets `srcObject = new MediaStream([track])`, pins jitter buffer if fields exist, `play()` on next user gesture.

- [ ] **Step 1:** `ontrack` branches on `track.kind`; audio must not set `gotVideoTrack`.
- [ ] **Step 2:** Hidden `<audio autoplay playsInline>` in `App.tsx`; click-to-play (R8).
- [ ] **Step 3:** `npm test` + `npm run build`.
- [ ] **Step 4:** Commit `feat(web): play host Opus from the audio transceiver`

### Task 5: Run R.* then I.*

- [ ] **Step 1:** Full regression list R.1–R.5.
- [ ] **Step 2:** Record R.6 baseline if not already saved.
- [ ] **Step 3:** I.1 → I.7 in order. Stop at first fail; do not stack "fixes."
- [ ] **Step 4:** Commit only if I.2 and I.3 pass. If I.2 fails drop%, ship `COUCHLINK_AUDIO=0` as default and open a follow-up — do not merge a video regression.

---

## Execution notes

- Implement on a branch; do not mix with the uncommitted roster/KBM viz work.
- First live enable: one friend, then three. Same invite, hard-refresh for JS.
- Rollback: `COUCHLINK_AUDIO=0` on the host process, no game restart.

Plan complete. Implementation is **not** in this commit.
