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

### 1.1 Make WireGuard the enforced default; cloudflared strictly last resort

Tonight's sessions ran with `COUCHLINK_SKIP_MESH=1` for most of the debugging,
which meant every restart fell straight to a `cloudflared` quick tunnel. That
was expedient for isolating the WSL networking bugs, but it is not the shape
this project wants running by default. `README.md:18` already states the
intended order — Headscale, then Tailscale/WireGuard, then TURN, then
cloudflared last — but tonight lived at the bottom of that list far more than
it should have.

Doesn't cost latency in any number we've measured — signaling is not the media
path. **What it is costing is real though, and worth hating: a new random URL
every restart, a third party sitting in your handshake,** and a dependency
that has already broken the Noise handshake once tonight and had to be worked
around. `scripts/enable-wireguard.sh` is already a direct, point-to-point
tunnel (`wg0-host.conf`, no relay in the data path) — nobody in the middle at
all, unlike Headscale (control-plane dependency) or cloudflared (full
in-path third party). It's the right thing to prefer; it just isn't being
reached for.

- [ ] Stop reaching for `COUCHLINK_SKIP_MESH=1` as the everyday debugging
      habit — it was necessary while chasing the networking-mode bugs, and
      those are fixed now. Test the real default path going forward.
- [ ] Confirm `run.sh`'s fallback order actually tries WireGuard *before*
      falling to cloudflared, not just Headscale before cloudflared. Trace it
      end to end — this was never explicitly verified tonight.
- [ ] Once mirrored networking is confirmed stable, check whether Headscale's
      own control-plane dependency is worth keeping ahead of WireGuard in that
      order, or whether direct WireGuard should be tried first now that WSL
      can hold real addresses.
- [ ] `cloudflared` stays wired in, but strictly as the fallback for a host
      with no usable direct path at all — never the everyday default.
- [ ] **Predict:** stable invite URLs across restarts once WireGuard is
      actually the live path; no third party in the handshake for the common
      case.
- [ ] **Refuted if:** WireGuard setup itself turns out to need the same
      per-restart churn (new keys, new config) — then the win is smaller than
      it looks and the real fix is making *that* stable instead.

---

## Part 2 — the topology-independent work (this is the real list)

**The premise:** a friend with no IPv6, behind a strict NAT, on a mediocre link
should still get a better experience after this work. Everything here holds
regardless of which of the three routes the triage found.

Ordered by expected gain per unit of risk.

### 2.0 Single-path video send — implemented, awaiting measurement

**Status: built on `perf/single-path-video-send` (merging to main alongside
FEC below).** No session was active when this was implemented, so there is no
before/after from the real instrument yet — that is the first thing to capture
next time someone connects.

The client now reports which path it paints (`present_path` over signaling)
and the host stops writing the other one (`crates/host/src/webrtc_peer.rs`,
`path_flags`). Unknown/unreported still sends both, so no viewer can go black
because we guessed. This composes with FEC below: `present_path` decides
*whether* the DataChannel is sent, `COUCHLINK_FEC` decides *how* it's encoded
when it is — independent knobs, same `push_h264`.

- [ ] Get the `before` number: on `main` pre-merge, watch for
      `frame push exceeded 50ms` rate and p95 `jitterBufferMs` on a live session.
- [ ] Reproduce the same session, same preset, post-merge.
- [ ] Compare. **Predict:** fewer/no `exceeded 50ms` events, p95 improves more
      than p50 (removes self-inflicted congestion, not steady-state delay).
- [ ] **Refuted if:** no measurable change — then the two streams were not
      meaningfully contending, and this is safe-but-inert.

### 2.1 Spend the spare bandwidth on FEC — implemented, awaiting measurement

**Status: built and tested on `perf/clvd-fec-single-loss-recovery`, PR #17.
Not yet measured against a live friend — the mechanism is proven, whether it's
worth turning on isn't.**

**The observation nobody had used yet:** utilisation is **0.29**. Roughly 70%
of the uplink sits idle while the pipeline pays latency for reliability. A
dropped fragment on the unordered, unreliable CLVD channel cost a full
keyframe request — a multi-frame stall for the round trip plus the next IDR.

Implemented: `encode_fragments_with_fec()`
(`crates/proto/src/video_frame.rs`) appends one XOR-parity fragment per
multi-fragment access unit — `frag_idx == frag_count`, one slot past the last
data index. Any single missing data fragment reconstructs as `XOR of the
rest, XOR parity`, no round trip. Two or more losses in one access unit still
fall through to the existing keyframe path. Ported to the client
(`web/src/clvd.ts`, `ClvdAssembler`). Wire-compatible: an assembler that
predates this treats the extra index as out of range and ignores it — no
protocol negotiation needed, the fragment is simply inert on old code.

Gated behind `COUCHLINK_FEC=1`, **default off** — enabling this without a
measured loss rate spends bandwidth on a problem that may not exist. Every
recovery path is unit-tested (8 new Rust tests, 6 mirrored TS tests): every
single-fragment-loss index including the variable-length last fragment, the
two-loss case provably never fabricating output, and single-fragment access
units correctly skipping parity (nothing to XOR against). Runtime-verified
that the wiring itself doesn't panic (`WebRtcHost::new()` runs at host
startup, before any player joins) — that's boot safety, not end-to-end proof.

- [ ] Measure loss rate on the media path first (`packetsLost` in the same
      `getStats()` we already poll).
- [ ] If loss > ~0.1%, `COUCHLINK_FEC=1` and compare against the same session
      without it.
- [ ] **Predict:** p95 falls sharply, p50 barely moves — FEC removes tail
      events, not steady-state delay. Fewer/no `frame push exceeded 50ms`
      events triggered by keyframe stalls.
- [ ] **Refuted if:** loss is ~0. Then retransmission is never triggered and
      FEC is pure overhead — leave `COUCHLINK_FEC` unset.

### 2.2 Kill the keyframe spike (periodic intra-refresh) — refuted as scoped

**Checked, not built.** Searched the vendored `windows` crate's Media
Foundation bindings (`crates/capture-bridge/src/mf_encoder.rs` already uses
`ICodecAPI`) for an intra-refresh control GUID. None exists in that surface —
Media Foundation's generic `ICodecAPI` doesn't expose a standard
`CODECAPI_AVEncVideoIntraRefreshMode`-equivalent; that capability lives in
vendor-specific NVENC/AMF/QuickSync APIs, not the generic MF wrapper this
code goes through.

- [ ] If ever revisited: bind the vendor API directly (NVENC on the dev
      machine's RTX 5080) rather than through MF's generic surface. Real scope
      increase — separate FFI surface, hardware-specific — not a quick add.
- **Refuted:** the generic MF path this codebase uses cannot do this. Recorded
  so nobody re-searches the same GUID list and reaches the same dead end.

### 2.3 Send slices, not whole frames — API confirmed, build blocked here

**Checked, not built — genuinely implementable, but not from this session.**
The same MF bindings search that refuted 2.2 found the opposite result for
slicing: `CODECAPI_AVEncSliceControlMode` and `CODECAPI_AVEncSliceControlSize`
are both present in the vendored `windows` crate
(`windows-0.56.0/.../MediaFoundation/mod.rs`). The API to emit multiple
slices per frame exists and is reachable from the same `ICodecAPI` the
encoder already uses for `CODECAPI_AVLowLatencyMode`.

Why not built now: this touches `crates/capture-bridge` (Windows-only,
requires the MSVC toolchain to compile — reachable via the existing
`powershell.exe` build pipeline, not blocked) **and** the CLVD wire format to
express a partial frame, **and** correctness here can only really be verified
by watching real decoded video — something this session had no way to confirm
headlessly. Shipping a wire-format change to the frame the browser paints,
unverified by an actual decode, is a different risk tier than the FEC change
above (which degrades to today's behavior if COUCHLINK_FEC is unset).

- [ ] Confirm slice count/size behavior with `CODECAPI_AVEncSliceControlMode`
      set, by inspecting encoder output on the Windows side directly.
- [ ] Extend CLVD framing to express partial frames.
- [ ] **Predict:** ~one frame-time (up to 16.7 ms) off p50.
- [ ] **Refuted if:** decoder-side buffering re-serialises it anyway.
- [ ] **Do this with a friend online** so the video result can actually be
      watched, not just built.

### 2.4 Make the game's clock the master clock — partially already true

**Correction to the original model, found by reading the code rather than
re-deriving from the naive 60Hz-everywhere assumption.** The "five 60Hz
boundaries → ~40ms quantisation" estimate in
`2026-08-06-latency-model-and-experiments.md` assumed every boundary polls at
frame rate. `crates/host/src/main.rs`:

```rust
let tick = if capturer.is_preencoded() {
    Duration::from_millis(2)   // 500Hz, not 60Hz
} else {
    Duration::from_millis(1000 / preset.fps.max(1) as u64)
};
```

The **production path is pre-encoded** (Windows GPU H.264, confirmed by
tonight's host logs: "GPU-encoded on Windows"). For that path the host
already polls the capture socket at 500Hz. Average wait for a frame that just
became available: ~1ms, not the ~8.3ms a naive 60Hz metronome would cost. One
of the plan's five boundaries was already collapsed before this session
started — it just hadn't been checked against the actual running code.

What's left, honestly:

- **This send-side collapse only matters for the pre-encoded path.** The raw
  (non-pre-encoded) path still ticks at `preset.fps`, i.e. genuinely 60Hz —
  the original estimate stands there.
- **The remaining unknown is upstream of this codebase entirely**: how DWM
  composition hands frames to `couchlink-win-capture.exe`'s WGC session. That
  happens inside `crates/capture-bridge` / the Windows compositor and is not
  observable from the Rust host at all without instrumenting the capture
  binary itself — a live-friend-independent measurement, but a real one, not
  done tonight.

- [ ] If the raw path is ever the one in use (no GPU encode), apply the
      phase-lock idea there — that boundary is still a real ~60Hz metronome.
- [ ] Instrument `couchlink-win-capture.exe`'s own frame-arrival timestamps
      (WGC callback to socket-write) to find the one quantisation source this
      session couldn't reach. Doesn't need a friend, does need a Windows build.
- **Refuted (partially):** "make the game's clock the master clock" as
  originally scoped assumed a problem that's already half-solved for the path
  actually in production use. Scope any further work to the two items above,
  not a rewrite of an already-fast poll loop.

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
