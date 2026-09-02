# Image quality / framerate / input-latency optimization — plan

Status: **plan only, nothing implemented**. Written from real `host_stats` and browser
telemetry captured live tonight (2026-08-15), not guesses.

## The evidence, first

Every debug-tab snapshot from tonight's session shows the same signature, regardless of
player or preset:

| Stage | Typical value | Share of budget |
|---|---|---|
| Capture (Windows→WSL handoff) | 4–5ms | **dominant** — flagged as bottleneck every time |
| Scale | 0.0ms | none (pre-encoded path skips it) |
| Encode | 0.0ms | none (GPU-encoded on Windows, host only relays) |
| Push (network send) | 0.1–3.1ms | small |

And yet: `Push rate` sat at **14–16fps** against an **encoder target of 60fps** (720p60
preset), with the encoder periodically stepping itself down to `1280×720@15`. Paint FPS on
the browser side tracked the same 10–16fps. RTT was 64–128ms on `srflx→srflx` (a real
internet path, not LAN) with drops as high as 14% in one window.

That combination — a *tiny* per-stage capture cost, but a *collapsed* actual framerate —
is the whole finding. This is not a "the encoder is too slow" problem or a "GPU can't keep
up" problem. Something outside the four measured stages is eating the rest of every frame's
budget, and the link governor is reacting to it by stepping quality down, which is treating
a symptom that isn't where it's being measured.

## Applying the discovery process, briefly

**The system, plainly:** a game runs on a Windows box. Something reads its picture at up to
60 times a second and hands each picture, already video-compressed, to a second program
running in a different, cordoned-off environment on the same physical machine (WSL). That
second program forwards the compressed picture, unmodified, to up to three other computers
over the internet, and those other computers turn the compressed picture back into a moving
image about a tenth of a second after it left the game.

**The invariant that matters:** total outbound bits/sec is bounded by the host's own
internet uplink, and — critically — that bound does **not** grow with player count, because
every player watches the identical picture (relabeling symmetry: swap any two players and
the video each should receive doesn't change). The push stage is fanned out to N peers
concurrently already (`push_to_all`, `futures_util::join_all`); measured push cost per
frame (0.1–3ms) confirms that fan-out isn't the leak.

**The state-variable question:** the four measured stages don't sum to anything close to
what the observed framerate implies (60fps target ⇒ 16.7ms budget; measured stages total
under 8ms even in the worst case). A quantity is missing from the model. The obvious
candidate given the architecture: the boundary crossing between "Windows captures/encodes"
and "the host relays," which is a network hop over the WSL virtual switch (`0.0.0.0:9876`,
a real TCP socket, not shared memory) — and that hop is measured as part of `capture_ms`
only for the *successful* path. Frames that get **shed** at that boundary (buffer full,
socket backpressure) don't show up as `capture_ms` at all — they show up as fewer
successful frames per window, which the host logs *as* framerate. The "capture" number is
honest about frames that got through; it says nothing about frames that didn't.

## Locksmith reframe

The conventional framing is a trilemma: quality, framerate, and latency trade off against
each other, and you can only push one by spending the other two. That framing is wrong for
*this* problem, and worth discarding before optimizing anything.

**Reframe the reality:** this isn't a compression problem at all. The GPU encode is already
free (0.0ms, confirmed). The actual constraint is a *transport* problem disguised as a
video problem — a TCP socket across a WSL/Windows boundary that two processes on the *same
physical machine* are using to talk to each other as if they were on separate machines. The
wall isn't "encode faster" or "compress harder"; it's "stop paying network-stack cost for
what is fundamentally same-host IPC."

**The outsider loop:** don't optimize the socket — replace the primitive. WSL2 and Windows
share physical RAM under Hyper-V. A named pipe or a shared-memory ring buffer between
`couchlink-win-capture.exe` and the host process crosses zero network stack, zero TCP
handshake/backpressure semantics, and zero WSL vEthernet NAT hop — it's the door thinking
it's a window: the two sides never needed a network protocol, they needed a handoff, and a
handoff has no reason to go through `0.0.0.0:9876` at all.

**The system fix:** stop treating "Windows→WSL handoff" as a tunable and start treating it
as a removable step. Once frames move over shared memory instead of a socket, there is no
"drop under backpressure vs. count as capture_ms" ambiguity left to chase — the failure
mode this whole investigation exists to explain disappears structurally, the same way the
5-controllers-for-4-people bug disappeared once identity was tied to slot instead of
connection order (same shape of bug: a transport-arrival-order concept masquerading as
domain state).

## Ranked optimization candidates

1. **Instrument the actual leak before touching anything (do this first, always).**
   Add a counter on both sides of the `0.0.0.0:9876` socket: frames Windows *sent* vs.
   frames the host *received and pushed*. Today's `dropped_frames`/`drop_pct` in
   `host_stats` only counts drops **after** capture succeeds (inside `push_bounded`'s
   budget). If the Windows-side sender is also shedding — which the arithmetic above
   suggests — that's currently invisible. This is a few hours of work (a counter and a log
   line on each side) and it turns every later step from a guess into a measurement.

2. **Replace the capture socket with a local IPC primitive** (named pipe or shared-memory
   ring buffer) between `couchlink-win-capture.exe` and the host, matching the reframe
   above. This is the single highest-leverage change if step 1 confirms the leak is there —
   it removes an entire class of loss rather than tuning around it. Real, scoped work:
   touches `crates/capture-bridge`'s writer and `crates/host/src/capture/bridge.rs`'s
   reader; the wire *format* (H.264 NAL + metadata) doesn't need to change, only the
   transport.

3. **Simulcast, from the earlier 4-player brainstorm, still stands.** One shared bitrate
   target punishes every player for whoever has the worst link (confirmed tonight: RTT
   ranged 64–128ms across simultaneous sessions). Two fixed tiers (e.g. 720p60 / 480p30),
   each still a single GPU encode, let each peer's answer pick the tier its own measured
   headroom supports — without multiplying encode cost by player count.

4. **Only after 1–2 are measured and shipped:** reconsider the link governor's step-down
   thresholds. Right now it's reacting to a symptom (push-stage drops) that may not be
   where the real loss is. Retuning it before fixing the transport risks tuning against the
   wrong signal and then having to re-tune once the real fix lands.

Bitrate/resolution knobs, encoder preset changes, and similar "turn a dial" ideas are
deliberately *not* in this list — every measured stage (capture, scale, encode) is already
near-zero cost. There's no dial left to turn on the encode side; the leak is elsewhere.

## Regression discipline — no regressing quality, framerate, or input latency

**Baseline, captured before any change lands:**
- `host_stats` over a 5-minute idle-desktop + 5-minute active-gameplay window: fps,
  dropped_frames, drop_pct, capture_ms, push_ms, target_width/height/fps/bitrate.
- Browser telemetry over the same window, per connected player: `decodeFps`,
  `jitterBufferMs`, `packetLossPct`, `freezeCount`, paint fps, and pad send-rate Hz (input
  latency proxy — a real drop here is the first sign a change costs CPU on the input path
  too, not just video).
- Save both as the numeric floor. A regression is any of: fps down, drop_pct up,
  jitterBufferMs up, freezeCount up, or pad Hz down, each by more than measurement noise
  (~5%) — not "it felt worse."

**Per-phase gate**, matching the plan's own recommended order:
1. Land the frame-accounting instrumentation (step 1) alone first — it changes nothing
   about behavior, only visibility. Regression check: identical to baseline (proves the
   counters themselves add no overhead).
2. Land the IPC transport change (step 2) behind a flag if practical, so it can be A/B'd
   against the current TCP path in the same session. Regression check against baseline,
   *and* against the new frame-accounting counters from step 1 — the real proof this
   worked is "frames sent == frames received" where before there was a gap.
3. Land simulcast (step 3) only after step 2's numbers are stable for a full session.
   Regression check per tier, not just the top one — a regression in the *fallback* tier
   for a poor-link player is exactly what step 3 exists to prevent, so it needs its own
   verification, not just "the good-link player still looks fine."
4. Only then revisit governor thresholds (step 4), with before/after host_stats from a
   session with an intentionally degraded link (e.g. throttled via `tc` or a VPN hop) to
   prove the retuned thresholds react correctly rather than just react differently.

Each phase needs its own live multiplayer test with real controllers before merging past
that phase — consistent with how every fix tonight was actually verified, not assumed.

## Domain of validity / what this plan doesn't cover

This plan is written from tonight's specific pipeline: pre-encoded GPU H.264 on Windows,
relayed unmodified by the host. If a future session runs the raw-BGRA / local-encode path
instead (no Windows GPU encoder available), the `encode_ms`/`scale_ms` numbers won't be
near-zero anymore and the ranked list above would need re-deriving against that path's own
`host_stats` — the method (measure, don't guess; reframe before tuning) carries over, the
specific numbers don't.
