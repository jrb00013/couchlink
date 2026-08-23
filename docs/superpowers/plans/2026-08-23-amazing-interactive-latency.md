# Amazing interactive latency — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn Ricardo’s *playable* session into a *felt* step-change: WebCodecs-default present, honest **input→photon**, and (if measured) remove the Windows↔WSL socket-shaped handoff — without reopening the IDR/push death spiral.

**Architecture:** Separate **input clock** from **presentation clock**. Video path = CLVD + WebCodecs latest-frame-wins; RTP = rescue only. Pad seq/timestamps watermark frames so the drawer optimizes button→picture. Capture transport becomes shared-memory when vsock wait is proven dominant.

**Tech Stack:** `couchlink-host`, `couchlink-win-capture`, Hyper-V/SHM bridge, CLVD v3→v4 (or sidecar), `webCodecsCanvas.ts`, pad DataChannel, DebugDrawer, existing age_echo / ricardo_playable_ab.

**Design:** `docs/superpowers/specs/2026-08-23-amazing-interactive-latency-design.md`

## Global Constraints

- Never reintroduce: 20ms P-budget, 1s keyframe budget, IDR-on-every-timeout, full dual-send when `present_path=webcodecs`.
- Keep `ricardo_playable_ab` + sacred path_flags/fps-hold/floor≥1250 green.
- Do not kill `couchlink-ds-vhid` or close PCSX2 to test.
- Photon UI must label estimation honesty until clocks are unified.
- SHM behind env/feature flag with Hyper-V fallback.

---

## File map

| Pillar | Files |
|---|---|
| WebCodecs-default | `web/src/App.tsx`, `web/src/player.ts`, `web/src/webCodecsCanvas.ts` |
| Input→photon | `crates/proto` (pad + video), `crates/host/src/webrtc_peer.rs`, `web/src/player.ts`, `web/src/inputPhoton.ts`, `DebugDrawer.tsx` |
| Present governor | `web/src/presentAge.ts`, `webCodecsCanvas.ts` |
| Handoff / SHM | `crates/host/src/capture/*`, `crates/win-capture/**`, `crates/capture-bridge/**` |
| Regressions | `ricardo_playable_ab.rs`, new `amazing_latency_*.rs` / `*.test.ts` |

---

## Task 1: WebCodecs-default (felt path)

**Why:** Canvas@74fps felt good; WebCodecs+LFW is the lower-latency design and was underused.

- [ ] **1.1** Log one structured reason whenever present stays `canvas` after 3s (`no_au` / `decoder_fail` / `fallback_timer` / `ua_legacy`).
- [ ] **1.2** Ensure stall → `warmup` → next paint re-promotes (no permanent `preferRtpPresent` without WebCodecs unavailable).
- [ ] **1.3** Acceptance: Chrome join → `present_path=webcodecs` + LIVE HUD within 3s.

### Regressions — T1

| ID | Case | Pass |
|---|---|---|
| T1-R1 | `promoteWebcodecs` after paint | one `present_path=webcodecs` |
| T1-R2 | Stall then paint | re-promote; not stuck warmup |
| T1-R3 | No VideoDecoder | canvas/rtp; no black |
| T1-R4 | Sacred S1 | `path_flags(WEBCODECS)=(false,true)` |
| T1-R5 | Live Chrome | present webcodecs &lt;3s |

---

## Task 2: Input→photon v1

**Why:** Pad is already 250Hz — underutilized without frame correlation.

- [ ] **2.1** Extend pad wire **compatibly** (version bump or trailing optional fields): `client_ts_ms` (u32 or f64 JSON sidecar on first N Hz sample — prefer binary additive if CLPD version allows).
- [ ] **2.2** Host: per-slot `last_input_seq` + `last_client_ts`; stamp into CLVD (`input_wm` u32) via **v4 header** or parallel map keyed by `seq` if header freeze preferred short-term.
- [ ] **2.3** Client: on paint, if watermark ≥ sent seq, `photon_ms = paint - client_ts_of(wm)`; show p50 in drawer.
- [ ] **2.4** Soft hold: one missed pad tick keeps digital buttons; document no stick prediction yet.
- [ ] **2.5** Acceptance: mashes show photon p50; moves with RTT.

### Regressions — T2

| ID | Case | Pass |
|---|---|---|
| T2-R1 | Old pad frame still decodes | host applies without panic |
| T2-R2 | Watermark monotonic | frame N wm ≥ N−1 |
| T2-R3 | Photon fixture | known Δms |
| T2-R4 | Hold one tick | R2 stays pressed |
| T2-R5 | Live | photon p50 ≤ RTT+45ms on Ricardo-class path |
| T2-R6 | Mixed old/new clients | session joins |

---

## Task 3: Presentation governor hardening

- [ ] **3.1** HUD: `LIVE · {fps} · {age}ms · photon {p50}ms · decode {ms}`.
- [ ] **3.2** Age-budget: already in `presentAge.ts`; ensure canvas + webcodecs both feed drawer.
- [ ] **3.3** Never wait for older missing frame (LFW invariant tests stay).

### Regressions — T3

| ID | Case | Pass |
|---|---|---|
| T3-R1 | `shouldReplacePending` | newer wins |
| T3-R2 | ageBand thresholds | ok/warn/drop/emergency |
| T3-R3 | Drawer shows photon when samples exist | not `—` while pad active |

---

## Task 4: Handoff truth → SHM

- [ ] **4.1** Counters: win-capture **frames_sent**, host **frames_received**, log every 5s with wait/copy split (A instrumentation).
- [ ] **4.2** Decision gate: if `wait_ms` p95 &gt; 1.0 **or** sent−recv gap material → implement SHM ring (same NAL payload).
- [ ] **4.3** `COUCHLINK_CAPTURE_IPC=shm|hyperv|tcp` select; default hyperv until SHM proven.
- [ ] **4.4** Acceptance: playable night capture stage drops **or** proof vsock already &lt;0.5ms p95 (then skip SHM).

### Regressions — T4

| ID | Case | Pass |
|---|---|---|
| T4-R1 | Spec parse shm/hyperv/tcp | invalid errors |
| T4-R2 | Hyper-V fallback when shm unavailable | host still streams |
| T4-R3 | No IDR storm on capture blip | coalesce |
| T4-R4 | Live | sent≈recv; wait p95 documented |

---

## Task 5: Amazing gate (north star)

- [ ] **5.1** Add `amazing_latency_ab` tests mirroring design bars (photon formula, webcodecs default flags, IPC mode enum).
- [ ] **5.2** Live checklist comment template for PR: photon p50, present mode, handoff wait, sacred suite.
- [ ] **5.3** Only then consider bitrate/1080 climb (old workstream F).

### Regressions — T5 / sacred

| ID | Case | Pass |
|---|---|---|
| S-all | `ricardo_playable_ab` | 7/7 |
| S-host | full host unit | 0 fail |
| S-web | age/present/photon vitest | 0 fail |
| AMAZE-1 | Live photon p50 ≤ RTT+45ms | friend drawer |
| AMAZE-2 | present=webcodecs | Chrome |
| AMAZE-3 | No death-spiral logs | no 1Hz keyframe budget spam |

---

## Execution order

```text
T1 WebCodecs-default     ← felt path (days)
T2 Input→photon v1       ← north-star metric (days)
T3 Present HUD harden    ← makes T2 visible (hours)
T4 Handoff counters→SHM  ← structural remove wait (week if needed)
T5 Amazing gate + climb  ← only after T2 live pass
```

---

## Domain of validity

- Chrome/WebCodecs friends; Safari stays RTP.
- Ricardo-class WAN (~40–80ms RTT). LAN will beat the bar easily.
- PCSX2 + persistent ViGEm unchanged.

## Failed guesses to remember

| Guess | Why it felt “barely” |
|---|---|
| Chase push_ms / push fps | Already ~0 / ~76 |
| Age-only without input watermark | Optimizes display freshness, not interactivity |
| Bitrate climb first | Quality ≠ input→photon |

---

## Done when

Friends say the session feels *snappier* (not just “still playable”), drawer shows photon, Chrome is on WebCodecs, and sacred + Ricardo A/B stay green.
