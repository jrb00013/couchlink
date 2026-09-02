# Amazing Interactive Latency — Math-Driven Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make **surplus** \(S = \Phi - R\) (input→photon minus RTT) the measurable objective, lock its formulas in `input_photon_budget` (same discipline as `wan3_math`), then wire watermarking / WebCodecs / handoff gates so live \(S_{p50}\) can move — without reopening the IDR/push death spiral.

**Architecture:** Applied-math first: \(\Phi\) on one client clock; objective \(S=\Phi-R\); phase waits \(T/2\); SHM only if handoff wait \(w\) is a material fraction of \(T_v\). Code changes exist to *observe and attack* terms in the budget identity, not to chase push fps.

**Tech Stack:** `crates/host/src/input_photon_budget.rs`, `couchlink-proto` CLPD/CLVD, `web/src/inputPhoton.ts`, WebCodecs present path, capture handoff counters.

**Design:** `docs/superpowers/specs/2026-08-23-amazing-interactive-latency-design.md`  
**Math:** `docs/superpowers/specs/2026-08-23-amazing-interactive-latency-math.md`  
**Prior (non-math) plan:** `docs/superpowers/plans/2026-08-23-amazing-interactive-latency.md` — superseded for execution order by **this** plan; keep as file-map reference.

## Global Constraints

- No constant without a measurement path (math doc rule).
- Never reintroduce: 20ms P-budget, 1s keyframe budget, IDR-on-every-timeout, full dual-send when `present_path=webcodecs`.
- `ricardo_playable_ab` 7/7 stays green.
- Do not kill `couchlink-ds-vhid` or close PCSX2.
- UI label: `input→photon (est.)` until clocks unified.
- Optimize **\(S_{p50}\)**; bitrate climb only after live \(S\) gate.

---

## File map

| Math / responsibility | File |
|---|---|
| Budget identity, bars, phase waits | Create: `crates/host/src/input_photon_budget.rs` |
| Module hook | Modify: host `main.rs` / `lib` cfg include (mirror `wan3_math`) |
| CLPD v2 `client_ts_ms` | `crates/proto/src/pad_frame.rs`, `web/src/clpd.ts` |
| CLVD v4 `input_wm` | `crates/proto/src/video_frame.rs`, `web/src/clvd.ts` |
| Host stamp wm | `crates/host/src/webrtc_peer.rs` |
| Client \(\Phi\), \(S\), p50 | `web/src/inputPhoton.ts`, App / DebugDrawer |
| WebCodecs-default (cut \(T_d/2\)) | `web/src/App.tsx`, `presentPromote.ts` |
| Handoff \(w\) / SHM gate \(\omega=w/T_v\) | `capture/mod.rs`, `hyperv_bridge.rs`, win-capture |

---

### Task 0: Lock the mathematics (`input_photon_budget`)

**Why first:** Same as `wan3_math` — formulas and bars live in one module with hand-worked tests. Wiring without this re-invents fudge numbers in UI strings.

**Files:**
- Create: `crates/host/src/input_photon_budget.rs`
- Modify: wherever `mod wan3_math` is declared — add `mod input_photon_budget;`

**Interfaces (produce):**

```rust
/// Ricardo playable-night RTT (ms). Source: session ~2026-08-23 02:08.
pub const RICARDO_RTT_MS: f64 = 48.0;

/// First wow bar: S* = Φ − R ≤ this (ms). Design + math doc.
pub const WOW_SURPLUS_MS: f64 = 45.0;

/// Stretch after handoff wait proven small / SHM.
pub const STRETCH_SURPLUS_MS: f64 = 30.0;

pub fn period_ms(fps: u32) -> f64;           // reuse pattern from wan3_math
pub fn mean_phase_wait_ms(fps: u32) -> f64; // T/2

/// S = Φ − R. Negative clamped in UI; tests allow raw for diagnosis.
pub fn surplus_ms(phi_ms: f64, rtt_ms: f64) -> f64;

/// η = S / R (dimensionless).
pub fn surplus_rtt_units(phi_ms: f64, rtt_ms: f64) -> f64;

/// Φ* = R + S*  (wow absolute photon at a given RTT).
pub fn photon_wow_ms(rtt_ms: f64) -> f64;

/// ω = w / T_v — handoff wait in video periods.
pub fn handoff_wait_periods(wait_ms: f64, video_fps: u32) -> f64;

/// SHM decision: wait p95 material if ω > 1/T fraction... use wait_ms > 1.0
/// OR ω > 0.06 (~1ms @60). Prefer absolute 1.0ms gate from design.
pub fn shm_gate_trips(wait_p95_ms: f64) -> bool;

/// Mean phase stack: pad + video + display (ms).
pub fn mean_phase_stack_ms(pad_hz: u32, video_fps: u32, display_fps: u32) -> f64;
```

- [ ] **Step 1: Write failing tests (hand-worked Ricardo example)**

```rust
#[test]
fn ricardo_wow_photon_is_rtt_plus_45() {
    assert!((photon_wow_ms(RICARDO_RTT_MS) - 93.0).abs() < 1e-9);
    assert!((surplus_ms(93.0, 48.0) - 45.0).abs() < 1e-9);
    assert!((surplus_rtt_units(93.0, 48.0) - 45.0 / 48.0).abs() < 1e-9);
}

#[test]
fn mean_phase_stack_at_60_and_250_is_about_18_7() {
    // T_p/2=2, T_v/2=8.333..., T_d/2=8.333... → 18.666...
    let s = mean_phase_stack_ms(250, 60, 60);
    assert!((s - 18.666_666).abs() < 0.01);
}

#[test]
fn residual_after_phases_inside_wow_is_about_26_3() {
    let residual = WOW_SURPLUS_MS - mean_phase_stack_ms(250, 60, 60);
    assert!((residual - 26.333_333).abs() < 0.01);
}

#[test]
fn shm_gate_trips_above_one_ms_wait_p95() {
    assert!(!shm_gate_trips(0.4));
    assert!(shm_gate_trips(1.01));
}

#[test]
fn surplus_is_translation_invariant_in_phi_and_r() {
    // Symmetry: shifting both Φ and R by Δ leaves S unchanged.
    let s1 = surplus_ms(90.0, 40.0);
    let s2 = surplus_ms(100.0, 50.0);
    assert!((s1 - s2).abs() < 1e-9);
}
```

- [ ] **Step 2: Run — expect FAIL**

```bash
cargo test -p couchlink-host input_photon_budget -- --nocapture
```

- [ ] **Step 3: Implement module**

Mirror style of `wan3_math.rs` module docs: every constant cites math doc or live source; no wishes as assertions.

- [ ] **Step 4: Run — expect PASS**

```bash
cargo test -p couchlink-host input_photon_budget
cargo test -p couchlink-host ricardo_playable_ab
```

- [ ] **Step 5: Commit**

```bash
git add crates/host/src/input_photon_budget.rs crates/host/src/main.rs
git commit -m "$(cat <<'EOF'
feat(host): input_photon_budget — surplus S=Φ−R and wow bars

Lock Ricardo hand-worked budgets and SHM wait gate before wiring
watermarks; push_ms is not the objective.
EOF
)"
```

---

### Task 1: WebCodecs-default (attack display phase \(T_d/2\))

**Files:** `web/src/presentPromote.ts` (new), `web/src/App.tsx`, tests  
**Math link:** LFW + WebCodecs-default removes stale-frame display wait from \(S\).

- [ ] **Step 1:** Failing vitest for `classifyPresentStuck` (see prior plan Task 1).
- [ ] **Step 2:** Implement + ensure stall → `resumeWarmup` + re-promote (no permanent `preferRtpPresent` on 2.5s timer).
- [ ] **Step 3:** Log structured stuck reason after 3s if not webcodecs.
- [ ] **Step 4:** `npx vitest run src/presentPromote.test.ts`
- [ ] **Step 5:** Commit `feat(web): WebCodecs-default stuck taxonomy for phase-wait cut`

**Acceptance:** Chrome → `present_path=webcodecs` &lt;3s; sacred `path_flags(WEBCODECS)=(false,true)`.

---

### Task 2: Observe \(\Phi\) — CLPD v2 + CLVD v4 + client ring

**Math link:** \(\Phi = t_p - t_s(\mathrm{wm})\); without wm, \(S\) is undefined.

**Wire (locked):**
- CLPD v2: 35 bytes = v1 + `client_ts_ms` u32 LE (host accepts v1+v2)
- CLVD v4: header 30 = v3 + `input_wm` u32 LE (pad seq); decode accepts 2/3/4

**Files:** proto pad/video, `webrtc_peer.rs`, `clpd.ts`, `clvd.ts`, `inputPhoton.ts`, player, App, DebugDrawer

- [ ] **Step 1:** Failing proto tests `v2_round_trips_client_ts_ms`, `v4_round_trips_input_wm`, `v1_still_decodes`, `v3_wm_zero`.
- [ ] **Step 2:** Implement encode/decode.
- [ ] **Step 3:** Host: on pad decode store `last_input_wm = max_monotonic(seq)`; stamp every CLVD AU.
- [ ] **Step 4:** Client `inputPhoton.ts`:

```ts
export function surplusMs(phiMs: number, rttMs: number): number {
  return phiMs - rttMs; // mirror input_photon_budget::surplus_ms
}
export function notePadSent(perfNow, seq): void;
export function notePhotonPaint(paintPerf, inputWm): number | null; // Φ sample
export function photonP50Ms(): number | null;
export function surplusP50Ms(rttMs: number): number | null;
```

Vitest fixture: send seq=5 at t=100, paint wm=5 at t=190 → \(\Phi=90\); with R=48 → \(S=42\).

- [ ] **Step 5:** Drawer: `photon {p50}ms (est.) · S {s50}ms` using live RTT when available.
- [ ] **Step 6:** Soft hold one pad tick (digital buttons) — reduces false \(\Phi\) spikes from dropped polls.
- [ ] **Step 7:** Tests + commit

```bash
cargo test -p couchlink-proto
cargo test -p couchlink-host input_photon_budget ricardo_playable_ab
cd web && npx vitest run src/clpd.test.ts src/clvd.test.ts src/inputPhoton.test.ts
```

```bash
git commit -m "$(cat <<'EOF'
feat: watermark pad→frame so surplus S=Φ−R is observable

CLPD v2 client_ts + CLVD v4 input_wm; drawer shows Φ and S (est.).
EOF
)"
```

**Live gate:** \(S_{p50} \le 45\,\mathrm{ms}\) on Ricardo-class path (`surplus_ms(photon_p50, rtt)`).

---

### Task 3: Handoff wait gate (attack \(W_\mathrm{handoff}\) only if \(\omega\) large)

**Math link:** `shm_gate_trips(wait_p95)`; \(\omega = w/T_v\).

- [ ] **Step 1:** Unit test `parse_capture_ipc` + assert `shm_gate_trips` used in decision comment/helper.
- [ ] **Step 2:** Log every 5s: `frames_recv`, `wait_ms`, `copy_ms`, and `omega=handoff_wait_periods(wait, fps)`.
- [ ] **Step 3:** One live night: if `!shm_gate_trips(p95)` and sent≈recv → **skip SHM**, document proof in PR.
- [ ] **Step 4:** If gate trips → SHM ring behind `COUCHLINK_CAPTURE_IPC=shm` (separate commit series); Hyper-V fallback required.
- [ ] **Step 5:** Commit instrumentation first.

---

### Task 4: Adversarial validation + sacred lock

**Math link:** translation symmetry of \(S\); domain of validity; failed-guess record.

- [ ] **Step 1:** Extend `input_photon_budget` tests if any bar drifted; keep Ricardo hand-work.
- [ ] **Step 2:** Optional live check (conjecture): LAN vs WAN — \(S\) should be closer than \(\Phi\) (log both; do not fail CI on conjecture).
- [ ] **Step 3:** Full suite:

```bash
cargo test -p couchlink-host
cd web && npx vitest run
```

- [ ] **Step 4:** PR checklist:

```text
MATH-1 ricardo_wow Φ*=93 at R=48
MATH-2 live S_p50 ≤ 45ms (wow) — stretch 30 after handoff proof
MATH-3 present=webcodecs <3s Chrome
MATH-4 shm_gate decision documented (trip or skip)
MATH-5 no death-spiral / ricardo_playable_ab 7/7
MATH-6 drawer shows Φ and S (est.), not push as hero
```

- [ ] **Step 5:** Commit `test: amazing-latency adversarial checklist + math lock`

**Bitrate/1080 climb:** explicitly **blocked** until MATH-2 passes live.

---

## Execution order (math-driven)

```text
T0 input_photon_budget     ← formulas / bars / SHM gate (hours)
T1 WebCodecs-default        ← cut Td/2 in the budget
T2 Observe Φ → compute S    ← north-star metric live
T3 Handoff ω → SHM iff gate ← structural only if measured
T4 Validate + sacred        ← then quality climb
```

---

## Domain of validity

- Chrome WebCodecs friends; Safari RTP (different \(T_d\) model).
- \(\Phi\) undefined without watermark — do not show fake S.
- \(S \ge 0\) expected long-run; brief negatives = clock/stall artifact.

## Failed guesses (do not reopen)

| Guess | Revealed |
|---|---|
| push_ms objective | Wrong state variable |
| glass age = feel | Wrong endpoints |
| Absolute \(\Phi\) bar only | Breaks across RTT; use \(S\) |
| SHM before measuring \(w\) | Premature attack on possibly tiny term |

## Done when

Live drawer shows \(\Phi\) and \(S\); \(S_{p50} \le 45\,\mathrm{ms}\) on Ricardo-class; WebCodecs-default; math module + sacred green; SHM only if gate tripped.
