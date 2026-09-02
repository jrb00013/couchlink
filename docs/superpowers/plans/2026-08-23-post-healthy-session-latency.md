# Post-healthy-session latency — next wins

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After Ricardo’s healthy drawer (push ~0.1ms, 77fps, 5Mbps@60, shed 0%), cut the *next* dominant costs — capture handoff, missing age on canvas, WebCodecs promotion, and input→photon — without reopening the IDR/push death spiral.

**Architecture:** Keep the proven invariants (fps-hold, CLVD-only when WebCodecs healthy, 120ms IDR budget, no IDR-on-timeout). Add measurements first where age/input clocks are blank, then optimize the named bottleneck (Windows→WSL handoff), then interactive latency.

**Tech Stack:** `couchlink-host` (Rust), `couchlink-win-capture`, Hyper-V bridge, WebCodecs/`webCodecsCanvas.ts`, pad DataChannel, existing `age_echo` / `host_stats` / DebugDrawer.

**Baseline (Ricardo, 2026-08-23 ~02:08):**

| Signal | Value | Implication |
|---|---|---|
| Push | 0.1ms, 77.8fps, 0% shed | Do **not** retune push budget / IDR storm logic |
| Encoder | 1280×720@60 5.00 Mbps | Governor healthy at baseline |
| Bottleneck | capture (Windows→WSL) 1.7ms | Next video win |
| Present | **canvas** (RTP) | WebCodecs path unused tonight |
| Age p50/p95 | — | Age echo not on canvas path |
| Pad | 251Hz DualSense | Input clock already fast; no photon metric yet |
| RTT | 48ms srflx | Network floor; don’t chase as “lag bug” |

## Global Constraints

- Never reintroduce: 20ms P-budget, 1s keyframe budget, IDR-on-every-shed, dual full RTP+CLVD when `present_path=webcodecs`.
- Do not kill `couchlink-ds-vhid` or close PCSX2 to test.
- Regression tests must be runnable offline (`cargo test` / `vitest`) for logic; live drawer checks are acceptance gates.
- Prefer measurement → then change. No bitrate climb until capture + age are honest.
- Co-author session work with Ricardo / Hung when committing user-facing latency fixes (existing practice).

---

## File map

| Area | Primary files |
|---|---|
| A – Capture handoff | `crates/host/src/capture/hyperv_bridge.rs`, `bridge.rs`, `crates/win-capture/**`, `scripts/ensure-win-capture.sh` |
| B – Age on all paths | `web/src/ageEcho.ts`, `web/src/player.ts`, canvas present path in `App.tsx`, `crates/host/src/age.rs` |
| C – WebCodecs prefer | `web/src/App.tsx`, `web/src/webCodecsCanvas.ts`, `web/src/player.ts` `promoteWebcodecs` |
| D – Input→photon | `crates/proto` pad/video stamps, `web/src/player.ts` pad loop, host watermark on CLVD/`host_stats` |
| E – Slow-peer isolation | `crates/host/src/main.rs` `push_to_all`, `webrtc_peer.rs` |
| F – Optional climb | `link_gov.rs`, `proto/signal.rs` presets — **last** |

---

## Workstream A — Capture handoff (named bottleneck)

### Intent

`dominant_stage = capture` with ~1.7ms average is small per frame but is the only non-zero host stage. Reduce copies, stalls, and “received 0 from win-capture” gaps without touching the push path.

### Tasks

- [ ] **A1.** Instrument: split `capture_ms` into `bridge_wait_ms` vs `frame_copy_ms` (or log histograms) so we know if the cost is blocking read vs memcpy.
- [ ] **A2.** Audit Hyper-V path for double-buffer / extra `Vec` clones on the hot path; collapse to one owner buffer where safe.
- [ ] **A3.** Ensure win-capture stays attached (picker/source pin, respawn) so “received 0” windows don’t appear under load.
- [ ] **A4.** Acceptance: healthy 3-friend session shows capture_ms ≤ baseline and **no** multi-second `received 0` streaks; push stays ≪1ms, shed ~0%.

### Regression tests — A

| ID | Type | Case | Pass criteria |
|---|---|---|---|
| A-R1 | Unit | Hyper-V / bridge parse of `hyperv:port:vm-id` | Invalid specs error; valid specs select HyperV bridge |
| A-R2 | Unit | Stale-frame shed still requests IDR (existing bridge behavior) | Shedding N stale frames still calls IDR once (no storm) |
| A-R3 | Unit | `SET_TARGET` round-trip still applies fps-hold rungs only | Commanded target never drops fps when stepping bitrate |
| A-R4 | Integration (host log) | 60s session with 1–3 peers | `received` from win-capture > 0 every 5s window; no `win-capture not reachable` after attach |
| A-R5 | Live drawer | Ricardo-class path | Bottleneck may stay capture, but capture_ms p95 does not regress >2× vs 1.7ms baseline; push stays <5ms |
| A-R6 | Negative | Kill win-capture 10s then restore | Host respawns/reconnects; does **not** IDR-storm (≤1 IDR request / 750ms coalesce) |

---

## Workstream B — Age echo on canvas / RTP present

### Intent

Age p50/p95 was blank while Ricardo felt good on **canvas**. Wire honest capture→paint (or receive→paint) age on every present path so the next optimization isn’t blind.

### Tasks

- [ ] **B1.** On RTP/canvas paint (or `requestVideoFrameCallback` / periodic sample), emit `age_echo` when `stamp_us` is available; if RTP has no stamp, echo recv→paint only and document host-side skip for `stamp_us=0`.
- [ ] **B2.** Keep WebCodecs paint-time echo (already done); ensure one echo per seq (existing `echoAgeOnce`).
- [ ] **B3.** DebugDrawer: show age even when present mode is canvas; show `—` only when truly no samples.
- [ ] **B4.** Acceptance: drawer shows age p50/p95 within ~1–2 ticks on canvas **and** webcodecs.

### Regression tests — B

| ID | Type | Case | Pass criteria |
|---|---|---|---|
| B-R1 | Unit | `echoAgeOnce` dedupe | Same seq twice → one send |
| B-R2 | Unit | `stamp_us=0` skipped for host capture-age | No false age from v2/unknown stamps |
| B-R3 | Unit | Encode/decode age_echo JSON | Round-trip fields match `AgeEcho` proto |
| B-R4 | Unit | Canvas present path calls echo helper | Mock paint invokes echo once per sampled frame |
| B-R5 | Live | Join as canvas (force WebCodecs off / Safari path) | Age p50/p95 non-empty within 10s |
| B-R6 | Live | Join as webcodecs | Age still non-empty; no double-count storm (host age window stable) |
| B-R7 | Negative | Pad DC closed | Echo no-ops; no throw; stream continues |

---

## Workstream C — Prefer WebCodecs when healthy (without breaking Ricardo)

### Intent

Ricardo felt good on canvas; WebCodecs + latest-frame-wins is still the lower-latency design. Make promotion reliable, keep RTP as stall safety net only.

### Tasks

- [ ] **C1.** Diagnose why session stayed on canvas (fallback timer? first paint never? Safari UA?). Add one log line: reason not promoted.
- [ ] **C2.** Tighten fallback: don’t demote to permanent RTP if WebCodecs painted once; stall → warmup → retry webcodecs.
- [ ] **C3.** Confirm host `present_path=webcodecs` ⇒ RTP off (existing); stall ⇒ warmup dual briefly.
- [ ] **C4.** Acceptance: Chrome friends promote to webcodecs; canvas only if WebCodecs unavailable or after repeated stall.

### Regression tests — C

| ID | Type | Case | Pass criteria |
|---|---|---|---|
| C-R1 | Unit | `path_flags(PATH_WEBCODECS)` | `(false, true)` unless `COUCHLINK_RTP_FULL` |
| C-R2 | Unit | `path_flags(PATH_UNKNOWN)` / warmup | `(true, true)` |
| C-R3 | Unit | `promoteWebcodecs` after first paint | Sends `present_path=webcodecs` once |
| C-R4 | Unit | Stall handler | Sends `warmup`, does not leave path stuck on warmup forever after re-paint |
| C-R5 | Unit | Latest-frame-wins | Newer pending ts replaces older; older closed/dropped |
| C-R6 | Live Chrome | Fresh join | `present_path webcodecs` in console; host push stays healthy |
| C-R7 | Negative | `VideoDecoder` missing | Stays RTP/canvas; no black screen |
| C-R8 | Regression | Reproduce old warmup bug | Must **not** stay on warmup after first WebCodecs paint (host must not full dual forever) |

---

## Workstream D — Input → photon

### Intent

Pad is already ~250Hz. Add correlation so lag is measured as **input → displayed frame**, not only WebRTC RTT / push ms.

### Tasks

- [ ] **D1.** Proto: input sequence + client timestamp on pad frames (extend CLPD or sidecar JSON carefully — prefer additive field / version bump).
- [ ] **D2.** Host: record last applied input seq per slot; stamp outbound video AU with `input_watermark` (or reuse/extend CLVD header with care).
- [ ] **D3.** Client: on paint, if frame watermark ≥ last sent input, compute `input_to_photon_ms`; show in DebugDrawer.
- [ ] **D4.** Optional v1 prediction: hold digital buttons across ≤1 missing pad tick; **no** aggressive stick prediction until measured.
- [ ] **D5.** Acceptance: drawer shows input→photon p50 under play; value moves when RTT changes.

### Regression tests — D

| ID | Type | Case | Pass criteria |
|---|---|---|---|
| D-R1 | Unit | Pad encode/decode with seq+timestamp | Old clients ignore unknown bytes **or** version gate rejects cleanly |
| D-R2 | Unit | Watermark monotonic | Frame N watermark ≥ frame N−1 for same slot timeline |
| D-R3 | Unit | Photon calc | `paint_ms - input_client_ts` (clock domain documented); synthetic fixture = known ms |
| D-R4 | Unit | Hold-button extrapolate | One missed pad tick keeps R2 pressed; two ticks policy documented |
| D-R5 | Live | Mash face button | input→photon samples appear; p50 finite and < 2× RTT + 50ms (sanity band) |
| D-R6 | Negative | Viewer with no pad | No watermark spam; video unaffected |
| D-R7 | Compatibility | Mixed old/new friend builds | Session still joins; no pad apply panic on host |

---

## Workstream E — Slow-peer isolation (Hung vs Ricardo asymmetry)

### Intent

Applied-math invariant: fan-out wall time ≈ `max(peer)`. One slow friend must not re-pin everyone. Ricardo recovered; keep a guardrail.

### Tasks

- [ ] **E1.** Per-slot shed counters in push window; if one slot’s shed ≫ others, mark `trickle` (skip non-keyframes for that slot for N frames / until buffered_amount drops).
- [ ] **E2.** Stats: expose per-slot push/shed in host logs or `host_stats` extension (optional UI later).
- [ ] **E3.** Acceptance: artificial 500ms delay on one peer (or throttle) drops that peer’s fps, others stay ≥50fps push.

### Regression tests — E

| ID | Type | Case | Pass criteria |
|---|---|---|---|
| E-R1 | Unit | `push_to_all` concurrent | Slow peer timeout does not serialize (wall ≈ max, not sum) — mock clocks |
| E-R2 | Unit | Trickle mark | After M consecutive sheds on slot 2 only, slot 2 skips deltas; slot 1 still sends |
| E-R3 | Unit | Keyframe still attempted on trickle slot at IDR_INTERVAL | Join/resync not permanently black |
| E-R4 | Unit | No IDR storm from trickle sheds | Coalesce ≥750ms |
| E-R5 | Live / sim | 2 peers, one impaired | Healthy peer paint stays high; impaired may canvas/low fps |
| E-R6 | Negative | All peers healthy | No trickle marks; bitrate can stay at 5Mbps |

---

## Workstream F — Optional quality climb (last)

### Intent

Only after A–C are honest and E won’t punish the group. Try higher bitrate or 1080 on clean nights.

### Tasks

- [ ] **F1.** Document climb gate: shed <2% for N windows, age p95 under threshold, capture not starved.
- [ ] **F2.** Optional preset or governor ceiling bump (e.g. 7.5–10 Mbps @720p60) behind env flag.
- [ ] **F3.** Acceptance: climb does not recreate push bottleneck (push_ms stays <10ms).

### Regression tests — F

| ID | Type | Case | Pass criteria |
|---|---|---|---|
| F-R1 | Unit | Governor fps-hold | Climb/down never changes fps |
| F-R2 | Unit | Floor ≥1250 | Never returns 625 rung |
| F-R3 | Unit | Climb requires clean windows | `UP_AFTER_CLEAN_WINDOWS` unchanged or documented |
| F-R4 | Live | Env higher ceiling | 3 friends, push_ms <10, shed <8%; else auto-step down |
| F-R5 | Negative | Force dual RTP (`COUCHLINK_RTP_FULL=1`) at high bitrate | Expect push stress — documents why default stays CLVD-only |

---

## Execution order

```text
B (age on canvas)     — unblocks measurement, small blast radius
A (capture handoff)   — named bottleneck on healthy session
C (WebCodecs prefer)  — uses B’s age to prove win
E (slow-peer guard)   — insurance before more bitrate
D (input→photon)      — interactive metric + light hold
F (climb)             — only with gates green
```

Do **not** start F or raise encoder defaults until A-R5, B-R5/B-R6, and C-R6 pass on a real 2–3 friend night.

---

## Sacred regressions (always run before ship)

These caught tonight’s meltdown; keep green forever:

| ID | Case | Pass |
|---|---|---|
| S1 | `should_send_rtp` / `path_flags` webcodecs | CLVD-only |
| S2 | P-frame push budget timeout | No `request_keyframe` storm |
| S3 | Keyframe push budget | ≤120ms class; failed IDR does not immediately force another |
| S4 | `rungs_from` | fps constant; floor ≥1250 |
| S5 | `promoteWebcodecs` | Not stuck on `warmup` after first paint |
| S6 | Latest-frame-wins | `shouldReplacePending` / `shouldSkipDecode` unit tests |
| S7 | Live smoke | Push ≫30fps possible; drawer can show push≪10ms at 5Mbps@60 |

```bash
# Offline gate
cargo test -p couchlink-host --bin couchlink-host
cd web && npx vitest run src/presentAge.test.ts src/ageEcho.test.ts src/webCodecsCanvas.test.ts
```

---

## Domain of validity

- Plan assumes pre-encoded Windows GPU path + Hyper-V bridge (Ricardo’s topology).
- LAN-only sessions may already be capture-bound with tiny RTT — A still helps; D matters less until age exists.
- Safari / no-WebCodecs stays RTP/canvas forever (C-R7).
- Input→photon clocks are different domains (client monotonic vs host stamp); v1 may report receive→photon or RTT-adjusted estimate — label honesty in UI.

## Failed guesses to remember

| Guess | Failure | Lesson |
|---|---|---|
| Cut P-budget to 20ms | IDR storm | Budget must match fan-out physics |
| IDR-only RTP while “healthy” | Still taxed push | Healthy WebCodecs ⇒ RTP **off** |
| Optimize paint age while push dead | “ok” age, 2fps | Freshness ≠ arrival rate |
| 1s keyframe budget “safe for join” | max(peer)=1s → 1fps | Join reliability ≠ steady-state budget |

---

## Done when

1. Capture instrumentation explains the 1.7ms (or cuts it).
2. Age visible on canvas and webcodecs.
3. Chrome friends prefer webcodecs without black-screen regressions.
4. Input→photon number exists in the drawer.
5. Slow peer cannot pin healthy peers below ~50fps push in a controlled test.
6. Sacred regressions S1–S7 green.
