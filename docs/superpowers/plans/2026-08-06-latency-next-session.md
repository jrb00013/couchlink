# Latency: what's next (pick up here with a friend online)

Written to be resumable. Read this, do the five-minute triage, then work the
list. Companion to `2026-08-06-full-latency-optimization-plan.md` (the model)
and `2026-08-06-latency-model-and-experiments.md` (the derivation).

---

## Five-minute triage when a friend connects

The client now prints one line whenever the media route or its RTT changes:

```
media path { local, remote, family, protocol, relayed, rttMs }
```

Everything below branches on it. **Get this line first.**

| What it says | What it means | Where to work |
|---|---|---|
| `relayed: false`, `family: IPv6` | Best route the internet offers | Nothing left in routing — go to Part 2 |
| `relayed: false`, `family: IPv4` | Hole-punch succeeded | Routing is fine — Part 2 |
| `relayed: true` | Going through a relay | Check *whose* relay — see below |
| line never appears | No pair succeeded | Connection bug, not latency |

**If relayed, check whose relay.** Ours runs on the host itself, so a relayed
path lands on the same machine the media was headed to anyway — it costs
essentially nothing. A *third-party* relay is a genuine detour. Do not treat
"relayed" as automatically bad; treat it as "find out which."

**Also capture:** `rttMs` from that line. It is the first transit number
measured on the media path rather than inferred from a ping to an unrelated
host, and every estimate in the other two documents should be re-derived from
it.

---

## Part 1 — routing (only if the triage says so)

Do not spend effort here unless the triage line points at it. Shortening the
network path is the *least* portable kind of win: it depends on IPv6 existing,
on NAT behaviour, on the friend's carrier. None of that generalises.

If routing does need work, in order of leverage:

- [ ] Confirm both ends offered IPv6 host candidates at all.
- [ ] If forced onto a third-party relay, move the relay to the host (ours) or
      a box near the host, so the relay is the destination rather than a detour.
- [ ] Only then consider port forwarding for a direct IPv4 path.

---

## Part 2 — the topology-independent work (this is the real list)

**The premise:** a friend with no IPv6, behind a strict NAT, on a mediocre link
should still get a better experience after this work. Everything here holds
regardless of which of the three routes the triage found.

Ordered by expected gain per unit of risk.

### 2.0 Single-path video send — implemented, awaiting measurement

**Status: built on `perf/single-path-video-send`, not yet measured against a
live friend.** No session was active when this was implemented, so there is no
before/after from the real instrument yet — that is the first thing to capture
next time someone connects.

The client now reports which path it paints (`present_path` over signaling)
and the host stops writing the other one (`crates/host/src/webrtc_peer.rs`,
`path_flags`). Unknown/unreported still sends both, so no viewer can go black
because we guessed.

- [ ] Get the `before` number: on `main`, watch for `frame push exceeded 50ms`
      rate and p95 `jitterBufferMs` on a live session.
- [ ] Switch to this branch, reproduce the same session, same preset.
- [ ] Compare. **Predict:** fewer/no `exceeded 50ms` events, p95 improves more
      than p50 (removes self-inflicted congestion, not steady-state delay).
- [ ] **Refuted if:** no measurable change — then the two streams were not
      meaningfully contending, and this is safe-but-inert.

### 2.1 Spend the spare bandwidth on FEC

**The observation nobody has used yet:** utilisation is **0.29**. Roughly 70% of
the uplink is sitting idle while the pipeline pays latency for reliability.

Every lost packet currently costs either a retransmission (a full round trip —
~30 ms on the measured path) or a corrupted frame that waits for the next
keyframe. Both are latency events caused by *loss*, not by bandwidth.

Forward error correction inverts that trade: send redundancy up front, and
recover small losses **without any round trip at all**. It costs bandwidth we
demonstrably have, to buy latency we demonstrably lack.

- [ ] Measure loss rate on the media path first (`packetsLost` in the same
      `getStats()` we already poll).
- [ ] If loss > ~0.1%, add FEC to the CLVD path sized to cover it.
- [ ] **Predict:** p95 falls sharply, p50 barely moves — FEC removes tail
      events, not steady-state delay.
- [ ] **Refuted if:** loss is ~0. Then retransmission is never triggered and
      FEC is pure overhead. *Check before building.*

### 2.2 Kill the keyframe spike (periodic intra-refresh)

A full IDR is many times the size of a P-frame. On a paced link that burst is a
queueing event: it delays itself and everything behind it, and it recurs every
IDR interval. Worse, our own congestion guard *asks* for IDRs when things get
tight, which is exactly when the pipe can least afford one.

Intra-refresh replaces the periodic spike with a rolling band of intra-coded
macroblocks spread across many frames. Same recovery property, no burst.
Standard practice in game streaming for exactly this reason.

- [ ] Check whether the Windows GPU encoder exposes intra-refresh.
- [ ] If so, switch from IDR-on-interval to rolling refresh.
- [ ] **Predict:** bitrate variance collapses; p95 improves; periodic hitches
      every IDR interval disappear.
- [ ] **Refuted if:** the encoder has no intra-refresh mode — then fall back to
      capping IDR size or lengthening the interval.

### 2.3 Send slices, not whole frames

We currently wait for a complete encoded frame before sending. If the encoder
emits slices, each can go the moment it exists, and the decoder can start work
before the frame is complete. That removes most of one frame-time from the
pipeline, on every frame, for everyone.

- [ ] Confirm the encoder can emit multiple slices per frame.
- [ ] Send per slice; ensure CLVD framing can express partial frames.
- [ ] **Predict:** ~one frame-time (up to 16.7 ms) off p50.
- [ ] **Refuted if:** decoder-side buffering re-serialises it anyway.

### 2.4 Make the game's clock the master clock

The deepest structural fix, and the one that subsumes the phase-locking
experiment.

Right now every stage runs on its own metronome and we pay half a period per
boundary. The alternative is not "more metronomes, faster" — it is **one clock
for the whole pipeline**: the game's frame completion drives capture, which
drives encode, which drives send. Every downstream stage becomes event-driven
instead of polled, and the quantisation term collapses toward zero.

The existing metronome is deliberate — arrival-paced sending wobbled 20–60 ms
and inflated the receiver's buffer. So the answer is not to delete it but to
make it a **phase-corrected pacer**: driven by frame arrival, smoothed enough to
stay uniform, without adding a fixed half-period of wait.

- [ ] Measure the frame-arrival interval distribution first. Its spread decides
      whether this is possible at all.
- [ ] If spread ≪ frame interval, drive the pipeline from arrival with a
      phase-corrected pacer.
- [ ] **Predict:** removes most of the ~40 ms quantisation budget.
- [ ] **Refuted if:** arrival spread approaches a frame interval — then the
      metronome is load-bearing and must stay.

### 2.5 Remove the WSL↔Windows hop

Capture runs on Windows; the host runs in WSL; every frame crosses a TCP socket
between them, with a copy and a scheduler wakeup on each side. Small in bytes,
not free in time — and it is pure overhead that exists only because of where the
processes live.

- [ ] Measure it directly: timestamp on the Windows side, compare on arrival.
- [ ] If material, either run the host natively on Windows or move the transfer
      to shared memory.
- [ ] **Predict:** low single-digit ms, but on *every* frame and for everyone.
- [ ] **Refuted if:** the measured crossing is sub-millisecond — then leave it
      alone and stop thinking about it.

---

## Part 3 — the perceptual angle

Worth naming because it changes what "improve latency" means.

The player does not experience end-to-end delay. They experience **the gap
between their thumb moving and the screen responding**. Those differ, and the
second is attackable in ways the first is not:

- **Wake-on-input** (already planned): the frame after an input is the one that
  matters. Expediting it improves the felt number without touching transit.
- **Consistency beats mean.** A steady 60 ms feels better than a mean of 45 ms
  that spikes to 120. Every item in 2.1 and 2.2 attacks the tail, not the
  average — and the tail is what gets described as "laggy."

This is why `p95` belongs in every measurement below, not just `p50`.

---

## Order of work

1. **Stage 0 instrument** (`age` end-to-end) — still blocking, still unbuilt.
2. Triage line from a live friend → decides whether Part 1 matters at all.
3. Measure loss and frame-arrival spread — these two numbers gate 2.1 and 2.4.
4. Then work Part 2 in order, each with its prediction checked.

---

## Standing rules for this work

- **No optimisation merges without a before/after from the same instrument.**
  Four separate times this system's *instrument* was wrong rather than the
  system: an `ss` grep missing IPv6 brackets, RTP stats describing a path the
  viewer wasn't painting, `hostname -I` returning a WireGuard address, and
  `nat_1to1_ips` rewriting candidates to a retired IP. Each looked like a real
  finding.
- **Record refutations.** `mirrored networking will cut latency` was refuted:
  29.0 ms → 28.4 ms RTT, inside the noise. It fixed reachability and bought no
  speed.
- **State p50 and p95.** A change that improves the mean and worsens the tail
  has made things feel worse.
- **Nothing here beats transit.** Measured ~14 ms one way. If single-digit total
  latency is the goal, that is proximity, not code.
