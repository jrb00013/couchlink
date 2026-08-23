# Host-FPS Hold + IDR-Only Rescue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Friends see the host’s capture fps (preset, usually 60) on a 3-friend WAN, with felt submit-wait at 60 Hz and overlay age at a record low — without cutting fps to “fix lag.”

**Architecture:** `f_cmd = f_host` on every governor rung; only `bitrate_kbps` steps. WebCodecs friends keep `path_flags = (true, true)` but RTP carries **IDRs only** (a skipped-reference P-frame cannot decode). Safari (`PATH_RTP`) and unreported (`PATH_UNKNOWN`) still get every frame. Age stamp + wake-on-input already exist — do not rebuild them.

**Tech Stack:** `crates/host/src/link_gov.rs`, `crates/host/src/wan3_math.rs`, `crates/host/src/webrtc_peer.rs` `push_h264`, existing CLVD v3 / pad `AgeEcho` / win-capture `SET_TARGET` + `X`.

**Contributors:** Hung (HungH206), Ricardo (RiccoWrld), Ido (IdoCohen560).

**Depends on:** age stamp, wake-on-input, overlay `age_p50_ms` / `age_p95_ms` already on this branch. Companion math audit: `docs/superpowers/plans/2026-08-22-host-fps-hold-low-age.md`.

## Status

| Task | State |
|------|-------|
| 0. This plan | This file — implementation not started |
| 1. Math probes (fps-hold + IDR-only `U`) | Not started — **blocking** |
| 2. Governor: bitrate only; bit-capacity bench | Not started |
| 3. Sync `wan3_math::rungs_from` + ladder tests | Not started |
| 4. `should_send_rtp` (IDR-only vs Safari/unknown) | Not started |
| 5. Gate `write_sample` in `push_h264` | Not started |
| 6. Live 3-friend 60 Hz gate | Not started |

## Global Constraints

- If `rungs_from(baseline)` emits any rung with `fps < baseline.fps`, the task failed.
- `path_flags(PATH_WEBCODECS) = (true, true)` stays. Thin means skip `write_sample` on WebCodecs **P-frames**, not `send_rtp = false`.
- Never send a P-frame on RTP unless every reference since the last IDR was also sent on RTP. Every-Nth P is illegal.
- Do not force IDR on pad press. Wake is still “next AU now.”
- Do not put age / expedite on `video_dc`.
- Do not kill `couchlink-ds-vhid` or close PCSX2 to test.
- Do not implement 4:4:4 / x264 / B-frames.
- Off: `COUCHLINK_RTP_FULL=1` or `COUCHLINK_RTP_EVERY_N=1` restores every-frame RTP. `COUCHLINK_WAKE_ON_INPUT=0` still disables wake.
- Failure mode is the same picture + same pads, never “Waiting for host offer.”
- Default host preset is `1080p60` / 18 Mbps (`StreamPreset::P1080_60`). Live 720@15/2500 was a **720p60** baseline after the old ladder. fps-hold applies to whichever baseline `LinkGov::new` received.

---

## Math locked (do not re-derive; do not implement a different thin)

```
T = 1000 / f                          // ms
W_unsync = T / 2                      // only at an unlocked periodic handoff
φ = f_cmd / f_host                    // must be 1
U_full = N * 2 * R                    // old dual-send, ignores FEC
U_idr  = N * R + N * (8 * S_idr / T_gop) / 1000
                                      // kbps; S_idr = 60_000 bench bytes; T_gop = 1 s
FEC    = +14_028 B per AU iff S > 14_000
A      = host_now - stamp_push after pad echo   // RTT-like; floor ≈ 2 * 14 ms
L      = pad_fwd + vigem + emu + wgc + submit + enc + hop + relay
       + net_vid + assemble + decode + vsync    // felt; A is not L
```

Worked `N = 3`, `S_idr = 60_000`, `T_gop = 1`:

| Mode | φ | U (order) |
|------|---|-----------|
| 15@2500 full dual + FEC | 0.25 | ~20 Mbps (FEC on 20.8 kB AUs) |
| 60@2500 full dual | 1 | 15 Mbps |
| **60@2500 IDR-only RTP** | **1** | **3*2500 + 3*480 = 8940 kbps** |
| 60@10M full dual + FEC | 1 | ~81 Mbps — will shed |
| 1080p60@18M full dual | 1 | ~147 Mbps — will shed |

Host relay is already 2 ms (`main.rs` pre-encoded cadence). Remaining `T/2` is `win_capture` `next_submit`. Wake zeros one sleep. Logged `age` ignores `recv_ms` / `paint_ms` and is stamped in `push_h264`, not at WGC.

Governor sees **DC sheds / `PUSH_BUDGET` only**, not RTP bytes. After fps-hold it can only shrink `R`. The old `link_gov` bench that treats capacity as **24 encoder-fps** is the model `wan3_math` already refuted — Task 2 replaces it with a **bit-capacity** bench.

---

## File Structure

**Created:** none. No second encoder, no SVC, no shm, no new proto version.

**Modified**

- `crates/host/src/wan3_math.rs` — `host_uplink_idr_only_kbps`; `rungs_from` copy matches `link_gov`; tests
- `crates/host/src/link_gov.rs` — `rungs_from`; `persistent_shed_*`; bit-capacity bench
- `crates/host/src/webrtc_peer.rs` — `rtp_full_dual`, `should_send_rtp`, gate `write_sample`

**Not modified**

- `crates/proto/src/video_frame.rs` (header stays v3)
- `web/src/ageEcho.ts` / `clvd.ts` (already ship)
- win-capture wake `X` (already ships)
- `path_flags` match arms (values stay)

---

### Task 1: Math probes — fps-hold and IDR-only uplink

**Files:**
- Modify: `crates/host/src/wan3_math.rs`
- Test: same file (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `N_FRIENDS`, `P720`, existing `host_uplink_kbps(enc_kbps, n, paths) -> u32`
- Produces: `pub fn host_uplink_idr_only_kbps(enc_kbps: u32, n: u32, idr_bytes: u32, gop_s: f64) -> u32`
  - `idr_kbps = round((8 * idr_bytes / gop_s) / 1000)`
  - `return n * enc_kbps + n * idr_kbps`
  - `gop_s <= 0` treated as `1.0`

- [ ] **Step 1: Write the failing tests** at the bottom of `wan3_math.rs` `tests`:

```rust
#[test]
fn every_rung_from_p720_holds_60_fps() {
    for r in rungs_from(&P720) {
        assert_eq!(r.fps, 60, "fps-hold violated: {r:?}");
    }
}

#[test]
fn idr_only_rtp_is_under_full_dual_and_near_9mbps_at_2500() {
    let full = host_uplink_kbps(2_500, N_FRIENDS, 2);
    let thin = host_uplink_idr_only_kbps(2_500, N_FRIENDS, 60_000, 1.0);
    assert_eq!(full, 15_000);
    assert_eq!(thin, 8_940, "3*2500 + 3*480");
    assert!(thin < full);
}

#[test]
fn fec_off_when_mean_au_under_14k_at_60_2500() {
    let bytes = bits_per_frame(2_500, 60) / 8.0;
    assert!(bytes < 14_000.0, "60@2500 mean AU {bytes} must be one CLVD fragment");
    let bytes_15 = bits_per_frame(2_500, 15) / 8.0;
    assert!(bytes_15 > 14_000.0, "15@2500 mean AU {bytes_15} must trip FEC");
}
```

- [ ] **Step 2: Run**

```bash
cargo test -p couchlink-host --bins -- every_rung_from_p720_holds_60_fps idr_only_rtp fec_off_when_mean -- --nocapture
```

Expected: `idr_only_rtp` FAIL (fn missing). `every_rung` FAIL (`rungs_from` still has 30/15). `fec_off` PASS (uses existing `bits_per_frame`).

- [ ] **Step 3: Add only this function** (do not change `rungs_from` yet):

```rust
pub fn host_uplink_idr_only_kbps(enc_kbps: u32, n: u32, idr_bytes: u32, gop_s: f64) -> u32 {
    let gop = if gop_s > 0.0 { gop_s } else { 1.0 };
    let idr_kbps = ((8.0 * f64::from(idr_bytes) / gop) / 1000.0).round() as u32;
    enc_kbps.saturating_mul(n).saturating_add(idr_kbps.saturating_mul(n))
}
```

- [ ] **Step 4: Re-run the same command.** `idr_only_rtp` and `fec_off` PASS. `every_rung` still FAIL.

- [ ] **Step 5: Commit**

```bash
git add crates/host/src/wan3_math.rs
git commit -m "$(cat <<'EOF'
test(math): fps-hold probe and IDR-only uplink

EOF
)"
```

---

### Task 2: Governor steps bitrate only

**Files:**
- Modify: `crates/host/src/link_gov.rs` `rungs_from` (today lines 49–71)
- Modify: `crates/host/src/link_gov.rs` tests `persistent_shed_steps_down_to_trickle` and `benchmark_governor_vs_no_governor_on_saturated_link`

**Interfaces:**
- Consumes: `EncodeTarget { width, height, fps, bitrate_kbps }`
- Produces: `rungs_from(baseline) -> [baseline, R/2, R/4, R/8]` each with `fps: baseline.fps`, same width/height. Dedup if a step repeats.
- P720 example: `60/10000, 60/5000, 60/2500, 60/1250`

- [ ] **Step 1: Replace the floor assertion** in `persistent_shed_steps_down_to_trickle` and add:

```rust
#[test]
fn persistent_shed_never_drops_fps() {
    let mut gov = LinkGov::new(P720);
    for _ in 0..20 {
        gov.on_window(40, 100);
        assert_eq!(gov.current().fps, 60);
    }
    assert!(gov.current().bitrate_kbps <= 2_500);
    assert_eq!(gov.current().width, 1280);
    assert_eq!(gov.current().height, 720);
}
```

In `persistent_shed_steps_down_to_trickle`, **delete** `seen.last().unwrap().fps <= 15`. Keep “must reach the floor” equality with `rungs_from(&P720).last()`.

- [ ] **Step 2: Run**

```bash
cargo test -p couchlink-host --bins -- persistent_shed_never_drops_fps persistent_shed_steps_down -- --nocapture
```

Expected: `persistent_shed_never_drops_fps` FAIL (floor still 15 fps).

- [ ] **Step 3: Replace `rungs_from`:**

```rust
fn rungs_from(baseline: &EncodeTarget) -> Vec<EncodeTarget> {
    let mut rungs = vec![*baseline];
    let mut kbps = baseline.bitrate_kbps;
    for _ in 0..3 {
        kbps = (kbps / 2).max(1);
        let extra = EncodeTarget {
            fps: baseline.fps,
            bitrate_kbps: kbps,
            ..*baseline
        };
        if !rungs.iter().any(|r| *r == extra) {
            rungs.push(extra);
        }
    }
    rungs
}
```

- [ ] **Step 4: Rewrite `benchmark_governor_vs_no_governor_on_saturated_link` to bit capacity.** Do not use `CAPACITY_FPS = 24` as the thing the governor steps. Model:

```rust
fn window_bits(kbps: u32, fps: u32) -> u64 {
    // 100 ms window = fps/10 frames, CBR bits/frame = kbps*1000/fps
    let frames = u64::from((fps / 10).max(1));
    let bits_per = u64::from(kbps) * 1000 / u64::from(fps.max(1));
    frames * bits_per
}

#[test]
fn benchmark_governor_vs_no_governor_on_saturated_bits() {
    const CAPACITY_KBPS: u32 = 4_000; // one-path bit ceiling; 10 Mbps baseline exceeds it
    const WINDOWS: usize = 400;
    // no-gov: locked at P720 (10_000 kbps) — every window over capacity
    // with-gov: steps R to 5000 then 2500; fps stays 60; sheds fall once R <= capacity
    let mut gov = LinkGov::new(P720);
    let mut g_over = 0u32;
    for _ in 0..WINDOWS {
        let t = gov.current();
        assert_eq!(t.fps, 60);
        let produced = window_bits(t.bitrate_kbps, t.fps);
        let cap = u64::from(CAPACITY_KBPS) * 100; // kbps * 0.1 s * 1000 bits
        let shed = if produced > cap { 40 } else { 0 };
        if shed > 0 {
            g_over += 1;
        }
        gov.on_window(shed, 100);
    }
    assert_eq!(gov.current().fps, 60);
    assert!(gov.current().bitrate_kbps <= CAPACITY_KBPS);
    assert!(g_over < 400, "governor must stop overflowing after down-steps");
}
```

Remove or `#[ignore]` the old fps-capacity bench so it cannot force a 15 fps floor back in. Keep `benchmark_sets_target_wire_bytes_vs_detached_encoder` unchanged.

- [ ] **Step 5: Run**

```bash
cargo test -p couchlink-host --bins -- link_gov -- --nocapture
```

Expected: all `link_gov` tests PASS. `persistent_shed_never_drops_fps` PASS. Floor of P720 is `60/1250`.

- [ ] **Step 6: Commit**

```bash
git add crates/host/src/link_gov.rs
git commit -m "$(cat <<'EOF'
feat(gov): hold host fps; step bitrate only

EOF
)"
```

---

### Task 3: Keep `wan3_math::rungs_from` a byte-for-byte policy copy

**Files:**
- Modify: `crates/host/src/wan3_math.rs` `rungs_from` (today the 60→30→15 copy)
- Modify: every test in that file that names 15 fps, `LIVE_TRICKLE` as the P720 floor, or `rungs_from(&P720)[2].fps == 30`

**Interfaces:**
- Produces: same `rungs_from` body as Task 2 (copy, do not `pub use` across crates — `link_gov::rungs_from` stays private)
- `LIVE_TRICKLE` stays in the file as the **2026-08-22 live observation** (15/2500). It is no longer `rungs_from(&P720).last()`.

- [ ] **Step 1: Change these tests to the new contract** (exact names today):

| Old test | New assertion |
|----------|----------------|
| `live_trickle_is_the_p720_ladder_floor` | `rungs_from(&P720).last().fps == 60` and `bitrate_kbps == 1_250` |
| `p720_ladder_climbs_same_bits_30_before_spending_uplink` | rename purpose: rungs are `60/10000, 60/5000, 60/2500, 60/1250` |
| `first_climb_off_trickle_is_30_at_same_2500` | first climb off floor is `60/2500` (same fps, double bits) |
| `plan_b_30fps_same_bits_keeps_15mbps_uplink` | `host_uplink_idr_only_kbps(2500, 3, 60000, 1.0) == 8940` |
| `rungs_from_must_not_take_the_live_floor_as_baseline` | passing `LIVE_TRICKLE` as baseline must still keep `fps == 15` on every rung (fps-hold relative to *that* baseline) and must not invent 7 fps |

`every_rung_from_p720_holds_60_fps` from Task 1 must now PASS.

- [ ] **Step 2: Run the tests** and confirm they FAIL on the old `rungs_from` copy (or FAIL assertions). Then paste the Task 2 `rungs_from` body into `wan3_math.rs`.

- [ ] **Step 3: Run**

```bash
cargo test -p couchlink-host --bins -- wan3_math -- --nocapture
```

Expected: PASS, including `every_rung_from_p720_holds_60_fps`.

- [ ] **Step 4: Commit**

```bash
git add crates/host/src/wan3_math.rs
git commit -m "$(cat <<'EOF'
test(math): ladder copy holds fps; LIVE_TRICKLE is history not floor

EOF
)"
```

---

### Task 4: `should_send_rtp` — IDR-only on WebCodecs

**Files:**
- Modify: `crates/host/src/webrtc_peer.rs` (module-level fns next to `path_flags`)
- Test: `crates/host/src/webrtc_peer.rs` `controller_host_tests`

**Interfaces:**
- Consumes: `PATH_UNKNOWN: u8 = 0`, `PATH_WEBCODECS: u8 = 1`, `PATH_RTP: u8 = 2`
- Produces:
  - `fn rtp_full_dual() -> bool`
  - `fn should_send_rtp(keyframe: bool, path: u8, full_dual: bool) -> bool`
- Rule: `full_dual || path == PATH_RTP || path == PATH_UNKNOWN || keyframe`
  - Unreported path stays full dual so a silent Safari cannot sit on IDR-only RTP.
  - WebCodecs P-frames: false unless `full_dual`.
  - Every IDR: true.

- [ ] **Step 1: Write**

```rust
#[test]
fn should_send_rtp_idr_only_on_webcodecs_full_on_safari_and_unknown() {
    assert!(should_send_rtp(true, PATH_WEBCODECS, false));
    assert!(!should_send_rtp(false, PATH_WEBCODECS, false));
    assert!(should_send_rtp(false, PATH_RTP, false));
    assert!(should_send_rtp(false, PATH_UNKNOWN, false));
    assert!(should_send_rtp(false, PATH_WEBCODECS, true));
}

#[test]
fn webcodecs_path_keeps_rtp_flag_even_when_thin() {
    assert_eq!(path_flags(PATH_WEBCODECS), (true, true));
}
```

The second test already exists as `webcodecs_path_keeps_rtp_so_a_lost_idr_has_a_live_fallback` — do **not** duplicate it; keep that existing name and assertion.

- [ ] **Step 2: Run**

```bash
cargo test -p couchlink-host --bins -- should_send_rtp_idr_only -- --nocapture
```

Expected: FAIL (fn missing).

- [ ] **Step 3: Implement**

```rust
fn rtp_full_dual() -> bool {
    matches!(
        std::env::var("COUCHLINK_RTP_FULL").as_deref(),
        Ok("1") | Ok("true")
    ) || matches!(
        std::env::var("COUCHLINK_RTP_EVERY_N").as_deref(),
        Ok("1")
    )
}

fn should_send_rtp(keyframe: bool, path: u8, full_dual: bool) -> bool {
    full_dual || path == PATH_RTP || path == PATH_UNKNOWN || keyframe
}
```

- [ ] **Step 4: Re-run Step 2.** Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/host/src/webrtc_peer.rs
git commit -m "$(cat <<'EOF'
feat(rtp): IDR-only rescue predicate; Safari and unknown stay full

EOF
)"
```

---

### Task 5: Gate `write_sample` — do not clear `send_rtp`

**Files:**
- Modify: `crates/host/src/webrtc_peer.rs` `WebRtcHost::push_h264` (the `if send_rtp { ... write_sample ...}` block)

**Interfaces:**
- Consumes: `path_flags`, `should_send_rtp`, `rtp_full_dual`, `self.present_path`
- Produces: same `Result<bool>` shed contract as today. Skipping an RTP P-frame is **not** a shed. A shed is still “active present path did not carry the frame” (CLVD congested / not delivered).

- [ ] **Step 1: Write a comment-locked unit** next to the Task 4 tests:

```rust
#[test]
fn skipping_a_webcodecs_p_on_rtp_is_not_a_path_flag_cut() {
    assert_eq!(path_flags(PATH_WEBCODECS), (true, true));
    assert!(!should_send_rtp(false, PATH_WEBCODECS, false));
}
```

- [ ] **Step 2: Run** — PASS once Task 4 landed (predicate only). Then change `push_h264`.

- [ ] **Step 3: Minimal gate.** Today:

```rust
if send_rtp {
    // write_sample always
    delivered = true;
}
```

Replace with:

```rust
let path = self.present_path.load(Ordering::Relaxed);
let (send_rtp, send_dc) = path_flags(path);
// ...
if send_rtp && should_send_rtp(keyframe, path, rtp_full_dual()) {
    // existing PlayoutDelay + write_sample body unchanged
    delivered = true;
}
```

Do not move the CLVD block. Do not change `video_dc_congested`, FEC, or `stamp_us: crate::age::now_us()`.

If `send_rtp && !should_send_rtp(...)` and CLVD then delivers, `delivered` stays whatever CLVD set — a successful CLVD P-frame is still `Ok(false)` (not shed). If CLVD sheds that P, still `Ok(true)` and `request_keyframe()` as today.

- [ ] **Step 4: Run**

```bash
cargo test -p couchlink-host --bins -- path_flags should_send_rtp webcodecs_path rtp_path unknown_path expedite age_echo -- --nocapture
cargo test -p couchlink-host --bins -- --nocapture
```

Expected: PASS. Existing `webcodecs_path_keeps_rtp_so_a_lost_idr_has_a_live_fallback` still `(true, true)`.

- [ ] **Step 5: Commit**

```bash
git add crates/host/src/webrtc_peer.rs
git commit -m "$(cat <<'EOF'
feat(rtp): skip WebCodecs P-frames on RTP; keep the track alive with IDRs

EOF
)"
```

---

### Task 6: Live 3-friend 60 Hz gate

**Files:** none if Tasks 1–5 are green. Record numbers in the commit message.

- [ ] **Step 1:** Rebuild host + win-capture. `cd web && npm run build`. Friends hard-refresh. Do **not** kill `ds-vhid` / PCSX2.

- [ ] **Step 2:** Prefer `--preset 720p60` (or `COUCHLINK_PRESET=720p60`) for the same WAN as 2026-08-22. Default `1080p60`/18M will not hold three copies; if you start there, the governor must step **bitrate only** and overlay `target_fps` must stay 60.

- [ ] **Step 3:** Three minutes, three browsers, mix of pads/KBM. Overlay `target_fps` equals the preset fps the whole time.

- [ ] **Step 4: Pass all of**
  - present fps ≥ 55 (or ≥ 0.9 × preset) on each friend
  - `age_p50 < 45` ms, `age_p95 < 80` ms (RTT-like overlay; floor ≈ 28 ms)
  - no new sustained shed% > 8
  - zero `chunk too short`
  - pad Hz ≥ 100
  - force a CLVD stall (one friend toggle network briefly): picture unhides on RTP within **one GOP (~1–2 s)**, not a frozen last P-frame forever
  - no IDR storm in host logs after a single shed

- [ ] **Step 5: Fail and stop** if present fps collapses to ~15 while `target_fps` says 60 — silent DC shed, not a legal governor step. Drop `R` or check `VIDEO_DC_MAX_BUFFERED`. Do not reintroduce 15 fps rungs.

- [ ] **Step 6: Commit** (no code required)

```bash
git commit --allow-empty -m "$(cat <<'EOF'
chore(lag): 3-friend 60fps age p50=… p95=… present=…

EOF
)"
```

Paste the overlay snippet in the body. No merge of the implementation PRs if Step 4 fails.

---

## Risk register

| ID | Risk | Mitigation |
|----|------|------------|
| R1 | Safari black on IDR-only | `PATH_RTP` and `PATH_UNKNOWN` send every frame |
| R2 | 60@2500 looks like mush | Climb `R` after 8 clean 5 s windows; fps stays 60 |
| R3 | Full dual-send sneaks back | Default IDR-only; `idr_only_rtp_is_under_full_dual` |
| R4 | Someone cuts fps again | `every_rung_from_p720_holds_60_fps` + `persistent_shed_never_drops_fps` |
| R5 | `send_rtp = false` after paint | Forbidden (`b26cf34`) |
| R6 | Every-Nth P-frame “thin” | Illegal; hidden decoder storms IDRs |
| R7 | Old fps-capacity bench restores 15 fps | Task 2 deletes/ignores it |
| R8 | 1080p60@18M on 3-friend WAN | Step `R`, never `f`; tell the operator to use 720p60 for the live gate |

## Hard no's

- Step fps below `f_host` in `rungs_from`
- `path_flags` → `(false, true)` after first paint
- IDR-on-press
- Skip-reference P-frames on RTP
- Software x264 as a 60 fps experiment
- Kill `ds-vhid` to test

## Self-review

| Spec | Task |
|------|------|
| `φ = 1` | 1, 2, 3 |
| IDR-only `U` | 1 |
| Bitrate-only ladder | 2, 3 |
| Predictive-decode / no Nth P | 4, 5 |
| `path_flags` unchanged | 4, 5 |
| Safari / unknown full RTP | 4, 5 |
| Age/wake untouched | Global + Task 5 stamp line |
| Live 60 + age gate | 6 |
| Do not use overlay age as `L` | Task 6 wording |

No TBD. Implementation is **not** in this file.

Plan complete.
