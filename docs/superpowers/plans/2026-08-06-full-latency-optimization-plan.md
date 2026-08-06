# Full latency optimisation plan

Companion to `2026-08-06-latency-model-and-experiments.md`, which builds the
model. This one decides what to *do*, in what order, and what would prove each
step wrong.

---

## 1. Reframe the reality

The instinct is to make the stream smaller and the encoder faster. Both are
nearly irrelevant here, and the measurements say so:

| Quantity | Measured | What it rules out |
|---|---|---|
| Uplink capacity | ~35 Mbps | — |
| Stream at 720p60 | ~10 Mbps | Utilisation **0.29** — not congestion-bound |
| RTT to open internet | **28.4 ms** | One-way transit ~14 ms |
| RTT to own router | **1.83 ms** | Wi-Fi, not wire (~0.3 ms expected) |
| Client jitter buffer | 6–8 ms | Already small; little left there |
| Decode rate | 50–60 fps | Client is keeping up |

At utilisation 0.29, queueing theory says delay is nearly flat in load. **The
bitrate lever is already spent.** Anyone tuning bitrate from here is optimising
a term that isn't in the sum.

**The wall is not bandwidth. It is not even the network.** It is that a frame
crosses five or more independently-clocked boundaries, and each unsynchronised
crossing costs on average half a period — ~8.3 ms at 60 Hz, regardless of how
fast the work at that stage is. Five of those is ~40 ms of pure waiting against
~14 ms of physics.

**The question changes from "how do we send faster" to "how many times does a
frame wait for a bus it just missed."** Speed is bounded by the speed of light.
Phase is bounded by nothing but our willingness to align clocks.

---

## 2. The outsider loop

Three moves that attack phase rather than throughput. None require a faster
network, a better codec, or more bandwidth.

### 2a. Stop sending the same frame twice

The host writes every frame to **both** the RTP track and the CLVD DataChannel,
because it has no idea which one the viewer paints. Chrome paints the
DataChannel; Safari has no WebCodecs here and paints RTP.

This is not a bandwidth emergency at 0.29 utilisation, but it doubles the
per-frame send work and makes two streams compete inside one congestion
controller — self-inflicted jitter, which the receiver then absorbs as *delay*.

The lock isn't the pipe; it's that the sender never asked which door the viewer
uses. Have the client report its present path, then send one.

### 2b. Phase-lock capture to composition

We sample on a metronome that has no relationship to the game's frame clock, so
we land at uniformly random phase — 8.3 ms of average waiting for nothing.

But frame arrival timestamps are observable. Estimate the offset, then delay the
tick to fire *just after* composition instead of at a random point. Same rate,
same CPU, ~8 ms deleted.

The bus doesn't run faster. We stop arriving one second after it leaves.

### 2c. Wake-on-input

Input and video are currently strangers. Yet the instant a pad frame arrives we
hold information the video pipeline does not: **this next frame matters more
than the last hundred.**

Expedite the frame following input rather than waiting for the metronome. This
attacks input→response delay — the thing actually *felt* as "laggy" — precisely
where it is quantised. We are not changing the key. We are changing when the
lock is willing to turn.

---

## 3. The system fix

Every latency bug tonight shared one shape: **a number was believed without an
instrument behind it.** Four separate times the instrument was wrong, not the
system — an `ss` grep missing IPv6 brackets, RTP stats describing a path the
viewer wasn't painting, `hostname -I` returning a WireGuard address, and
`nat_1to1_ips` rewriting candidates to a retired IP.

The permanent fix is not any single optimisation. It is that **`age` becomes a
first-class observable**: capture time stamped into the frame, echoed back on
the pad channel, logged at paint. Clock offset cancels on the round trip, so no
clock sync is needed.

Once `age` exists, every claim in this document becomes falsifiable in one
session, and the whole class of "it feels laggy / it looks fine to me" arguments
stops being possible. That is the blocker that never comes back.

---

## Execution order

Sequenced by evidence required, not by size.

### Stage 0 — instrument (blocking)

- [ ] Stamp a monotonic capture timestamp into the CLVD header.
- [ ] Echo it on the pad channel; log `now − stamp` at paint.
- [ ] Report p50/p95 `age` per session.

**Gate:** nothing below merges until this reports numbers. Every latency figure
we currently hold came from `getStats()` on the RTP receiver while the viewer
painted from the DataChannel.

### Stage 1 — free wins, no measurement needed to justify

- [ ] **Size the emulator window to exactly 1280×720.** Capture was 1808×1080
      and 1920×1080, forcing a per-pixel scale every frame and producing
      `frameHeight: 738` — not even macroblock-aligned. Zero code.
- [ ] **Wire the host.** 1.83 ms RTT to your own router is Wi-Fi; Ethernet is
      ~0.3 ms, and more importantly it removes Wi-Fi's *variance*, which the
      receiver converts into buffer.
- [ ] **Oversample the host metronome.** Its rate is a free parameter; the
      quantisation is `T/2` whatever it is. 120 Hz halves that term.

### Stage 2 — single-path send (2a)

- [ ] Client reports present path (`webcodecs` | `rtp`) over signaling.
- [ ] Host sends only that path; falls back if the client re-reports.
- [ ] **Must survive** WebCodecs failing mid-session — the fallback has to
      re-arm, or a viewer goes black exactly like the Safari bug did.

**Predict:** small p50 gain, larger p95 gain (less self-inflicted congestion).
**Refuted if:** p95 is unchanged — then the two streams weren't interfering.

### Stage 3 — phase-lock (2b)

- [ ] Measure the WGC arrival-interval distribution **first**.
- [ ] If spread ≪ frame interval, estimate phase and offset the tick.

**Predict:** ~8 ms off p50 at unchanged rate and CPU.
**Refuted if:** arrival spread approaches 16.7 ms — phase estimation is then
noise, and this whole branch should be abandoned rather than tuned.

### Stage 4 — wake-on-input (2c)

- [ ] On pad-frame arrival, expedite the next capture frame.

**Predict:** input-to-visible p50 improves by roughly one host tick.
**Refuted if:** RPCS3's own poll interval dominates — a clock we do not own.

---

## Refuted along the way — recorded so nobody re-runs them

- **"It's bandwidth."** Measured 35 Mbps against ~10 Mbps of stream.
  Utilisation 0.29 killed the congestion story before any tuning happened.
- **"It's the friend's network."** Two friends, one host, one fine and one
  black. The asymmetry pointed at our relay, not their link.
- **"Mirrored networking will cut latency."** It did not: 29.0 ms → 28.4 ms
  RTT, inside the noise. It fixed *reachability* — the relay could finally
  answer — and bought no speed. Worth recording precisely because the change
  felt like it should have helped.
- **"coturn wasn't binding IPv6."** A bad `ss` grep missing `[addr]:port`
  brackets. coturn had been binding it all along.

---

## Domain of validity

- **Utilisation.** All of this assumes `ρ ≈ 0.29`. At 1080p, or with 2+
  players, `ρ` rises and queueing re-enters the sum. Re-measure before applying
  any of it there.
- **Loss.** The model ignores retransmission. The video channel allows a 100 ms
  retransmit window; at meaningful loss that term dwarfs everything here.
- **Client capability.** Decode is treated as one frame. On a weaker client it
  is more, and the client term is then underestimated.
- **Multiplayer.** Fan-out adds a per-peer send that is not in this model at
  all. See `2026-08-04-multiplayer-remote-pads.md`.

---

## What this plan will not do

It will not beat one-way transit, measured at ~14 ms. No reordering of clocks
makes light faster. If the goal is single-digit total latency, the answer is
physical proximity, not code — and saying so is part of the plan rather than a
failure of it.
