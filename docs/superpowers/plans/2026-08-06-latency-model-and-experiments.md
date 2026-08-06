# Latency: a model of the pipeline, and the experiments that would prove it

**Status:** model + experiment design. No optimisation should be merged from this
document until the instrument in Experiment 0 exists, because every latency
number we currently possess measures a path the viewer is not watching.

---

## System, in plain language

A picture appears on one screen. Some time later it appears on another. Someone
moves a stick; some time later the game reacts.

In between sits a chain of independent machines. Each wakes on **its own clock**,
does a little work, and hands the result to the next one. Nothing coordinates
those clocks.

That last sentence is the whole model.

---

## Inventory

**Entities:** the game, the Windows compositor, the capture bridge, the WSL host
process, the encoder, two NATs, the network, the browser, its decoder, its
canvas, the display, the controller, the virtual pad, the emulator's input poll.

**Events, with the clock each runs on:**

| Boundary | Clock | Delay contributed |
|---|---|---|
| Game renders → DWM composites | display refresh, 60 Hz | 0–16.7 ms |
| DWM → capture bridge takes a frame | capture cadence | 0–one period |
| Capture encodes → sends | GPU encoder | work, not waiting |
| Host cadence tick relays | fixed metronome | 0–one period |
| Network transit | continuous | one-way delay |
| Client jitter buffer | adaptive | measured 6–8 ms |
| Decode → canvas paint | frame-paced | ~one frame |
| Canvas → his display | his refresh, 60 Hz | 0–16.7 ms |

**Measurable quantities (units):** frame interval (ms), capture cadence (Hz),
host tick (Hz), bitrate (bit/s), uplink capacity (bit/s), one-way delay (ms),
jitter (ms), pad poll interval (ms), buffer occupancy (ms).

**Hard constraints:** one-way delay ≥ propagation; no stage can emit a frame it
has not received; a decoder cannot show a P-frame whose reference it lacks.

---

## The boring part, which is where the structure lives

The dramatic quantities — bitrate, resolution, dropped frames — are the ones
everybody tunes. Watch the boring ones instead: the delays that are *always*
there, in the steady state, when nothing is going wrong.

Every stage above is doing microseconds-to-milliseconds of actual work. Yet the
end-to-end delay is far larger than their sum. The difference is not
computation. It is **waiting for the next bus**.

---

## Candidate invariant

> Every boundary between two unsynchronised periodic processes contributes, on
> average, **half a period** of delay — independent of how fast the work is.

If a producer emits at uniformly random phase relative to a consumer polling
every `T`, the expected wait is `T/2`, uniform on `[0, T]`.

**Break attempts.**

- *Faster work?* Doesn't help. The term is `T/2` regardless of service time.
- *Producer faster than consumer?* Still `T/2` per crossing; frames are dropped
  rather than delayed less.
- *Phase-locked producer and consumer?* **This breaks it** — the wait collapses
  toward the fixed offset, which can be driven to ~0. That is not a flaw in the
  invariant; it is the exploit, and it is Experiment 2.
- *Jitter comparable to `T`?* Degrades to noise; the mean survives but the
  variance grows, which the buffer then absorbs as *more* delay.

So the invariant holds except under phase-locking, and phase-locking is exactly
the lever.

**Consequence.** With five 60 Hz boundaries, quantisation alone is
`5 × 8.3 ≈ 40 ms`. Measured RTT to the open internet from this host is **29 ms**,
so one-way transit is ~15 ms. **The waiting is larger than the physics.**

---

## Symmetry: what the answer cannot depend on

- **Time-translation.** No stage has a privileged origin, so delay must depend on
  *phase differences* between clocks, never absolute time. Any model with a
  wall-clock term in it is wrong.
- **Relabelling.** The stages are exchangeable with respect to total delay: only
  the multiset of periods matters, not their order. This is why reordering the
  pipeline cannot help, and why removing or slowing any one clock helps by the
  same rule wherever it sits.
- **Scale.** Halve every period and total quantisation halves. Delay is
  homogeneous of degree 1 in the periods — so there is no clever regime where
  raising rates stops paying; it just gets expensive.

Together these rule out any model of the form "delay = f(bitrate, resolution)"
as the *dominant* term. Bitrate enters only through transmission time and
congestion, both of which are small at 720p/10 Mbps on a 35 Mbps uplink.

---

## Dimensionless groups

Quantities: frame period `T`, one-way delay `D`, jitter `σ`, buffer `B`,
bitrate `R`, capacity `C`.

Fundamental units here: time, and information. Six quantities, two units → **four
independent dimensionless groups**:

| Group | Meaning | Measured here |
|---|---|---|
| `ρ = R/C` | uplink utilisation | 10/35 ≈ **0.29** |
| `β = B/T` | buffer in frames | 8 ms / 16.7 ms ≈ **0.5** |
| `j = σ/T` | jitter relative to a frame | unmeasured |
| `δ = D/T` | transit in frames | 15/16.7 ≈ **0.9** |

**Limiting cases pin the behaviour:**

- `ρ → 1`: queueing delay diverges. We are at 0.29, so **congestion is not the
  regime we are in** — which is why bitrate tuning gave so little.
- `j → 0`: the buffer can shrink to zero. `β ≈ 0.5` with a well-behaved link says
  there is little left to win in the buffer.
- `δ ≫ 1`: transit dominates and nothing local matters. We are at `δ ≈ 0.9` —
  transit and quantisation are *comparable*, so local structure is still worth
  attacking.

The reduction is the result: **we are not throughput-bound, we are phase-bound.**

---

## State variables

The obvious variables — fps, bitrate, dropped frames — fail the Markov test. Two
sessions with identical fps and bitrate behave differently, because what
determines the next frame's delay is *where in each clock's period it arrives*.

The summary of history that restores sufficiency is the **phase vector**: the
offset of each stage's clock relative to the frame's arrival. That is the
variable nobody in the domain names, and inventing it is the point of this
document.

Derived and more useful than any raw level:

- `φ_capture` — phase of the capture tick relative to DWM composition.
- `φ_host` — phase of the host metronome relative to encoded-frame arrival.
- `age` — time since the frame's pixels were true, carried *with* the frame.

`age` is the one to instrument: it is monotone along the pipeline, additive, and
directly the thing we want to minimise.

---

## Experiments — instrument before optimising

### Experiment 0 (blocking): measure `age` end to end

Nothing below is worth merging until this exists.

Stamp a monotonic capture timestamp into the CLVD header; echo it back on the pad
channel; log `now − stamp` at paint time. Clock offset cancels on the round trip,
so no clock sync is needed.

**Success:** a per-frame `age` distribution — p50, p95 — from the path the viewer
actually paints. Every claim below becomes falsifiable the moment this lands.

**Why it is blocking:** every latency number gathered so far comes from
`getStats()` on the **RTP receiver**, and the viewer has been painting from the
WebCodecs DataChannel. We have been reading an instrument attached to the wrong
pipe.

### Experiment 1: does the half-period law hold?

Vary the host tick 30/60/120 Hz, hold everything else fixed, measure `age` p50.

**Prediction:** p50 falls by ~8 ms going 60→120 Hz, ~17 ms going 30→60.
**Refuted if:** p50 is flat — then a queue, not quantisation, dominates, and the
whole model is wrong.

This is the cheapest possible discriminator between the two competing stories,
and it should be run first.

### Experiment 2: phase-lock capture to composition

Estimate `φ_capture` from arrival timestamps, then delay the tick to fire just
*after* composition rather than at uniform random phase.

**Prediction:** ~8 ms off p50 at unchanged rate and CPU.
**Refuted if:** WGC delivery jitter is comparable to a frame — then phase
estimation is noise. Measure the arrival-interval distribution *first*; if its
spread approaches 16.7 ms, skip this and say so.

### Experiment 3: wake-on-input

On pad-frame arrival, expedite the next capture frame instead of waiting for the
metronome.

**Rationale:** at that instant we hold information the video pipeline lacks —
this frame matters more than the last hundred. It attacks input→response delay,
which is what is actually *felt*, precisely where it is quantised.

**Prediction:** input-to-visible p50 improves by roughly one host tick.
**Refuted if:** the emulator's own poll interval dominates — bounded below by
RPCS3's input cadence regardless of what we do.

### Experiment 4: stop sending every frame twice

The host sends each frame on both the DataChannel and RTP because it cannot tell
which the viewer paints. At `ρ = 0.29` this is not a bandwidth emergency, but it
doubles encode-adjacent work and makes the two streams compete in one congestion
controller.

**Prediction:** small p50 gain, larger p95 gain (less self-inflicted congestion).
**Note:** requires the viewer to report its present path — a protocol change, and
the fallback path must survive WebCodecs failing mid-session.

---

## Simplifications, and what each costs

- **Independent uniform phases.** Real clocks partially couple (both derive from
  the same 60 Hz display), so true quantisation may be below `5 × T/2`. Cost:
  the 40 ms figure is an upper bound, not a point estimate.
- **Ignoring retransmission.** The video channel allows a 100 ms retransmit
  window; a lost fragment can add far more than any term modelled here. Valid
  only at low loss — check loss before trusting any of this.
- **Treating decode as one frame.** On a weak client it is more. Cost: the client
  term is underestimated for slower machines.

---

## Domain of validity — where this should fail

- **High utilisation.** As `ρ → 1` queueing dominates and the phase model becomes
  irrelevant. Re-measure `ρ` before applying any of this to 1080p or 2+ players.
- **Lossy links.** Retransmission and keyframe recovery swamp quantisation.
- **Mismatched refresh.** A 120 Hz client changes the last term; the *sum* rule
  survives but the arithmetic does not.
- **Multiplayer.** Fan-out adds a per-peer send that is not in this model at all.

---

## Failed guesses, and what each revealed

- **"It's bandwidth."** Measured uplink 35 Mbps against ~10 Mbps of stream —
  `ρ = 0.29`. Killed the congestion story and forced the phase model. Had this
  not been measured, days would have gone into bitrate tuning.
- **"It's the friend's network."** Two friends on one host, one fine and one
  black. Asymmetry pointed at *their* NAT and *our* relay, not at throughput —
  and the relay turned out to be advertising an address it could not serve.
- **"The client stats tell us the latency."** They describe the RTP path, which
  the viewer was not painting. This is what makes Experiment 0 blocking.

---

## Generalisation check

The half-period law is not specific to video. It applies to any chain of
independently-clocked stages — audio pipelines, sensor fusion, CI pipelines,
request/response systems with polling. Wherever someone has optimised
per-stage throughput and remains disappointed by end-to-end latency, the
quantisation sum is worth computing before touching any stage's speed.
