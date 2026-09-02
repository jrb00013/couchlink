# Math: Amazing interactive latency (input→photon)

**Status:** discovery draft locked for implementation  
**Date:** 2026-08-23  
**Companion design:** `docs/superpowers/specs/2026-08-23-amazing-interactive-latency-design.md`  
**Companion plan:** `docs/superpowers/plans/2026-08-23-amazing-interactive-latency-math-impl.md`

Every constant below is either measured (Ricardo 2026-08-23 playable night) or derived. No free fudge factors. Conjectures are labeled.

---

## System (plain language, no specialized vocabulary)

A person presses a button on their computer. That press is sampled many times a second and sent to another computer that is running the game. The game draws a new picture. That picture is copied, compressed, sent back across the network, decoded, and shown on the person’s screen. What the person *feels* is how long after the press the picture that *includes* the press’s effect appears. Tonight we already know the middle of that path is fast. We do not yet measure the whole button→picture time on one clock.

---

## Inventory

### Entities
- Friend’s browser (pad + display)
- Pad sample messages (250/s)
- Host (ViGEm → PCSX2 → capture → encode → send)
- Capture handoff (Windows process → WSL host)
- Access units (compressed pictures) with optional input watermark
- RTP spare path (warmup/stall only)

### Actions (discrete vs continuous)
- **Discrete:** button edge, pad send, AU encode, paint
- **Continuous / periodic:** pad period \(T_p\), capture/encode period \(T_v\), display refresh \(T_d\), network RTT

### Measurable quantities (with units)

| Symbol | Meaning | Unit | How measured |
|---|---|---|---|
| \(R\) | Round-trip time | ms | existing RTT / ICE stats |
| \(\Phi\) | input→photon | ms | client: paint − pad_send of watermarked seq (same `performance.now`) |
| \(S\) | surplus over RTT | ms | \(S = \Phi - R\) |
| \(\eta\) | surplus in RTT units | 1 | \(\eta = S / R\) |
| \(T_p\) | pad period | ms | \(1000/250 = 4\) |
| \(T_v\) | video period | ms | \(1000/\mathrm{fps}\) (60 → 16.67) |
| \(w\) | Hyper-V handoff wait | ms | host `take_handoff_ms` |
| \(a\) | glass age (encode→paint) | ms | existing age_echo / stamp |
| \(f_\mathrm{push}\) | push rate | 1/s | host (already ~76–78) |

### Constraints (always / never)
- Never beat light: one-way ≥ ~14 ms on WAN (already in `wan3_math`)
- \(\Phi \ge R\) in expectation for remote play (input must go up; picture come down) — **approximate** (clocks, buffering can violate briefly)
- When `present_path=webcodecs`, must not full-dual-send (sacred)
- `input_wm` monotone non-decreasing per peer
- Do not invent constants without a measurement path

---

## Representations

### Diagram (clock domains)

```
client clock ── pad send(seq,t_s) ──────────────────────────── paint(t_p)
                     │                                              ▲
                     ▼                                              │
host         last_wm=seq ──► AU.input_wm ──► network ──► decode ───┘
                     │
                     └── Φ := t_p − t_s(seq=wm)   [client only]
```

### Time series (Ricardo playable night — boring middle)

| Quantity | Value | Note |
|---|---|---|
| push | ~0.1 ms | spent lever |
| push fps | ~77.8 | spent lever |
| shed | 0% | healthy |
| encode | 720p60 @ 5 Mbps | |
| RTT | ~48 ms | physics |
| paint | ~74 | canvas path |
| \(\Phi\) | **unmeasured** | underutilized |

### Hand-worked example (first wow bar)

Given \(R = 48\,\mathrm{ms}\), design bar \(\Phi_{p50} \le R + 45\):

\[
\Phi^\star = 48 + 45 = 93\,\mathrm{ms},\quad S^\star = 45\,\mathrm{ms},\quad \eta^\star = 45/48 \approx 0.94
\]

**Phase-wait budget inside \(S\)** (unsync mean \(T/2\), from `wan3_math::mean_unsync_wait_ms`):

| Wait | Formula | @60fps | @250Hz pad |
|---|---|---|---|
| Video phase | \(T_v/2\) | 8.33 ms | — |
| Display phase | \(T_d/2\) | ~8.33 ms | — |
| Pad quantize | \(T_p/2\) | — | 2.0 ms |
| Sum of three means | | | **≈ 18.7 ms** |

Remaining for sim + capture copy + encode + decode + handoff ≈ \(45 - 18.7 = 26.3\,\mathrm{ms}\) before the wow bar fails. Stretch bar \(S^\star=30\) leaves only ~11 ms for that remainder → SHM / wake only if phase+handoff eats it.

---

## Candidate invariants

| Kind | Claim | Break attempt |
|---|---|---|
| Bounded | \(S_{p50}\) should stay roughly stable when \(R\) changes if host/client work fixed | LAN vs WAN A/B; if \(S\) tracks \(R\), metric is polluted |
| Monotone | `input_wm` never decreases for a peer | reconnect / wrap — use u32 wrap-aware “recent” |
| Structural | WebCodecs healthy ⇒ CLVD-only path_flags | sacred tests |
| Approximate | \(\Phi \ge R\) over long windows | short stalls may dip; p50 not min |
| Conserved (failed) | push_ms ≈ 0 is *not* conserved with felt lag | **failed guess:** optimizing push does not move \(\Phi\) |

---

## Candidate symmetries

| Symmetry | Implication |
|---|---|
| Time-origin (client) | \(\Phi\) must use one clock for send and paint — never mix host `stamp_us` into \(\Phi\) |
| Friend relabeling | Per-peer \(\Phi\); aggregate p50 across friends only for ops, not for one friend’s feel |
| Scale \(R\) | Objective is **\(S=\Phi-R\)**, not \(\Phi\) alone — removes absolute RTT from the optimization target |
| Description | Label UI `input→photon (est.)` until host/client clocks unified |

Rules out: using glass age \(a\) as a proxy for \(\Phi\) (different endpoints); optimizing \(f_\mathrm{push}\) as objective.

---

## Dimensionless groups

Relevant: \(\Phi, R, T_v, T_p, w\) (ms). One time unit → groups:

1. \(\eta = (\Phi - R)/R = S/R\) — surplus in RTT units (primary)
2. \(\sigma_v = S / T_v\) — surplus in video-periods
3. \(\omega = w / T_v\) — handoff wait in video-periods

**Limiting cases**
- \(R \to 0\) (LAN): \(\Phi \to S_\mathrm{local}\); bar becomes absolute \(S\) budget
- \(w \to 0\): SHM / perfect handoff; stretch \(S^\star=30\) becomes plausible
- LFW present: display phase wait → 0 for *stale* frames (not for causality)

**Wow bars as dimensionless**
- First: \(S_{p50} \le 45\,\mathrm{ms}\) (Ricardo-class), or \(\eta_{p50} \le 45/R\)
- Stretch: \(S_{p50} \le 30\,\mathrm{ms}\)

---

## Candidate state variables

| Variable | Markov? | Role |
|---|---|---|
| \(S_{p50}\) (or running p50 of \(S\)) | yes enough for ops | **true objective** |
| `present_path` | yes | which wait model applies |
| `last_input_wm` | yes | watermark state |
| push_ms, push_fps | yes but **wrong objective** | already near floor |
| glass age \(a\) | yes | display freshness ≠ interactivity |

History summary needed for \(\Phi\): ring of `(seq → t_s)` on client (finite; 256 samples ≈ 1s @250Hz).

---

## Is this optimization?

- **Decision variables:** present path (webcodecs vs rtp), whether to SHM, watermark on/off, soft-hold on/off — not bitrate first
- **Objective:** minimize \(S_{p50}\) (hard product feel); bitrate climb is soft / later
- **Hard constraints:** sacred path_flags, no IDR storm, ViGEm/PCSX2 stay up
- **Soft:** \(S\le45\), then \(S\le30\)
- **Information:** client knows \(t_s,t_p\); host knows wm; neither alone knows \(\Phi\) — needs watermark join

---

## Conceptual model

**Category:** additive delay budget with unsynchronized periodic waits (difference equation / budget identity), not a fluid PDE.

\[
\Phi \approx R + \underbrace{\tfrac{T_p}{2} + \tfrac{T_v}{2} + \tfrac{T_d}{2}}_{\text{mean phase waits}} + W_\mathrm{sim} + W_\mathrm{handoff} + W_\mathrm{enc} + W_\mathrm{dec} + \varepsilon
\]

\[
S := \Phi - R
\]

LFW / WebCodecs-default attack \(T_d/2\) and queueing; SHM attacks \(W_\mathrm{handoff}\); watermark makes \(S\) observable.

---

## Proof / plausibility

Claim: “push≈0 ⇒ felt lag fixed” is **false**. Survives Ricardo data (push floor, still want amazing).  
Claim: “\(S\) is the right objective” — plausibility: same-machine LAN should show smaller \(\Phi\) but similar \(S\) if model holds; **experiment before trusting**.

---

## Simplifications

| Drop | Cost |
|---|---|
| PCSX2 internal lag unknown | \(W_\mathrm{sim}\) lumped into residual |
| Stick prediction | digital soft-hold one tick only |
| Multi-friend coupled uplink in \(\Phi\) | per-friend \(\Phi\); uplink in wan3_math |

---

## Experiments before solutions

1. Instrument \(\Phi\), compute \(S=\Phi-R\) live
2. Confirm WebCodecs path (changes \(T_d\) term)
3. Measure \(w\) p95; SHM iff \(\omega = w/T_v\) material
4. Only then bitrate climb

---

## Domain of validity

- Chrome WebCodecs; Safari RTP (different present model)
- WAN RTT 40–80 ms Ricardo-class
- Fails if watermark missing (no \(\Phi\)); if client clock reset mid-session; if pad seq wrap without ring clear

---

## Failed guesses

| Guess | Why it failed |
|---|---|
| Optimize push_ms | Already ~0; invariant of health not feel |
| Optimize glass age alone | Wrong endpoints for interactivity |
| Bitrate first | Quality ≠ \(S\) |
| Absolute \(\Phi\) bar without \(R\) | Breaks across LAN/WAN; use \(S\) |

---

## Generalization

Same \(S\)-objective applies to native client later; SHM is one attack on \(W_\mathrm{handoff}\); wake-on-input attacks remaining \(T_v/2\) (already in wan3_math).
