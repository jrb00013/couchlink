# Design: Amazing interactive latency (post-playable)

**Status:** design locked → implement via companion plan  

**Date:** 2026-08-23  
**Context:** Ricardo’s session is already *playable* (push ~0.1ms, ~78fps, 5Mbps@60, 0% shed). Incremental fps/push tuning feels “barely.” Friends need a **felt** step-change.

---

## 1. Locksmith — reframe the reality

**Wrong question:** “How do we make push/fps numbers better?”  
Those levers are mostly spent. Push is already noise; paint ~74; RTT ~48ms is physics + path.

**Right question:** “How many independently-clocked handoffs does a button press wait on before photons move?”

Felt lag ≈  
`input_net + sim/emulation + capture_phase + handoff + encode_gop + transit + decode + present_phase`

Tonight’s healthy drawer only illuminates the *middle* of that chain. The underutilized ingredients already in the room:

| Already present | Underused as |
|---|---|
| Pad @ 250Hz | Not correlated to frames → no input→photon |
| CLVD `stamp_us` + age_echo | Blank on canvas; not wired to input |
| WebCodecs + latest-frame-wins | Ricardo stayed on **canvas**/RTP |
| Hyper-V vsock | Still a *socket* between two processes on one machine |
| Dual clocks (input vs video) | Coupled psychologically; not architecturally separated |

**Blocker that isn’t a wall:** “Make video arrive faster.”  
**Floor under the wall:** stop *waiting* — align clocks, remove a handoff, paint newest, measure button→picture.

---

## 2. Outsider loop (non-obvious path)

Don’t optimize the capture socket’s milliseconds. **Make the door think it’s a window:**

1. **Present clock ≠ network clock** — force WebCodecs + latest-frame-wins as the primary path so the player is a real-time display, not a media pipeline (RTP becomes warm spare only).
2. **Input clock ≠ video clock** — pad timeline + frame watermarks → report **input→photon**; then hold/extrapolate held buttons across one missed tick (prediction where it’s free).
3. **Same-machine handoff ≠ network** — replace Hyper-V/TCP frame transport with a **shared-memory / named-pipe ring** between win-capture and host so “capture bottleneck” ceases to be a network-shaped wait.
4. **One success metric** — optimize until `input_to_photon_p50` moves, not until push_ms moves.

---

## 3. Approaches considered

| Approach | Idea | Trade-off | Verdict |
|---|---|---|---|
| **A – Incremental** | More of post-healthy plan (age HUD, handoff split logs) | Safe; feels “barely” | Reject as primary |
| **B – Clock architecture (recommended)** | WebCodecs-default + input→photon + SHM handoff + presentation governor | Larger; measurable step-change | **Choose** |
| **C – Native viewer** | Skip browser present stack | Huge product surface | Defer |

---

## 4. Design (Approach B)

### Success criteria (amazing)

On a Ricardo-class path (~50ms RTT, 720p60, 1–3 friends):

1. Chrome present mode = **webcodecs** within 3s of join (not stuck on canvas).
2. Drawer shows **input→photon p50** continuously while pad active.
3. Target: **input→photon p50 ≤ RTT + 45ms** (≈ **≤ 95ms** at 48ms RTT) as a first “wow” bar; stretch **≤ RTT + 30ms** after SHM lands.
4. Host handoff: either SHM path live, or instrumented proof that vsock wait is &lt;0.5ms p95 (decide SHM from data).
5. Sacred regressions still green (no IDR storm, fps-hold, CLVD-only when healthy, Ricardo playable A/B).

### Non-goals

- Raising bitrate / 1080p as the hero lever.
- Touching ds-vhid / killing PCSX2 for tests.
- Full stick prediction / rollback netcode (v2).

### Architecture sketch

```
PAD 250Hz ──► DataChannel ──► ViGEm/PCSX2
                 │
                 └─ seq + client_ts ──► host input timeline
                                            │
GAME ──► win-capture ──► [SHM ring | vsock] ──► host ──► CLVD (stamp + input_wm)
                                            │
                                            ▼
                              WebCodecs latest-frame-wins ──► rAF present
                                            │
                                            └─ age_echo + photon calc ──► drawer
RTP: warmup/stall/Safari only
```

### Workstreams (ordered)

1. **P0 — Make WebCodecs the felt path** — diagnose canvas stickiness; re-promote; prove age + LFW on the path friends *see*.
2. **P0 — Input→photon v1** — pad seq/ts; host watermark on CLVD (header v4 or sidecar); client photon metric + soft button hold.
3. **P1 — Presentation governor** — already partly in LFW; harden age-budget drop policy; HUD “LIVE / age / photon”.
4. **P1 — Handoff truth then SHM** — sent vs received counters; if wait dominates, shared-memory ring (keep NAL format).
5. **P2 — Quality climb** — only after photon p50 is the north star and gates stay green.

### Risks

| Risk | Mitigation |
|---|---|
| CLVD header bump breaks old clients | Versioned header; old clients ignore wm |
| SHM complex on WSL2 | Feature-flag; keep Hyper-V fallback |
| WebCodecs stall → black | Warmup dual + hidden RTP rescue (already) |
| Photon clock domains confuse | Label “input→photon (est.)”; document domains |

---

## 5. Spec self-review

- No placeholders for success numbers — bars are explicit.
- Does not reopen IDR/push death spiral.
- Scope is one product goal (felt interactive lag), three technical pillars.
- F (bitrate climb) explicitly last.

---

## Approval

Companion math: `docs/superpowers/specs/2026-08-23-amazing-interactive-latency-math.md`  
**Execute:** `docs/superpowers/plans/2026-08-23-amazing-interactive-latency-math-impl.md` (math-driven; supersedes file-map-only plan for task order).  
File-map reference: `docs/superpowers/plans/2026-08-23-amazing-interactive-latency.md`.
