# Host-FPS Hold + Record-Low Age Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Friends see the **same fps the host is capturing** (preset, usually 60) and a lower glass-to-glass `age` than this repo has ever logged on a 3-friend WAN — without trading smoothness for snappiness.

**Architecture:** Stop packaging fps and bitrate in one governor step. Command `f = f_host` always; only `R` (kbps) moves. Keep RTP *alive* as an **IDR-only rescue** on the WebCodecs path (every P-frame on RTP is illegal — H.264 cannot decode a P whose reference was skipped). Safari (`PATH_RTP`) still gets every frame. Age + wake-on-input from PR #45 stay on: they skip leftover submit wait at 60 Hz, they do not replace 60 fps.

**Tech Stack:** `link_gov.rs` rungs, `push_h264` in `webrtc_peer.rs`, CLVD v3 + pad `AgeEcho` (PR #45 / `feat/wan3-age-wake-same-bits-30`), win-capture `SET_TARGET` + `X`, `wan3_math.rs`.

**Depends on:** PR #45 merged or this branch rebased onto it (age stamp, wake-on-input, overlay p50/p95). Do not re-implement those.

Companions: `docs/superpowers/plans/2026-08-22-input-lag-reduction.md`, `crates/host/src/wan3_math.rs`.

## Status

| Task | State |
|------|-------|
| 0. This plan | **This commit** — implementation not started |
| 1. Math: fps-hold invariant + thin-RTP `U` | Not started — **blocking** |
| 2. Governor: never step fps, only bitrate | Not started |
| 3. Thin RTP rescue (every Nth + all IDRs) | Not started |
| 4. Live 3-friend 60 fps + age gate | Not started |

## Global Constraints

- **Do not cut fps to “fix lag.”** If `rungs_from(baseline)` emits any rung with `fps < baseline.fps`, the task failed.
- **Do not cut RTP to zero** after WebCodecs paint. `path_flags(PATH_WEBCODECS) = (true, true)` stays. Thin means **IDR-only** samples on RTP for WebCodecs friends, not `send_rtp = false`, and not every-Nth P-frame.
- **Do not force IDR on pad press.** Wake-on-input is still “next AU now.”
- **Do not put age / expedite on `video_dc`.**
- **Do not kill `couchlink-ds-vhid` or close PCSX2** to test.
- **Do not implement 4:4:4 / x264 / B-frames** here.
- Off switches: `COUCHLINK_RTP_FULL=1` (or `COUCHLINK_RTP_EVERY_N=1`) restores full dual-send; `COUCHLINK_WAKE_ON_INPUT=0` still disables wake; age has no off switch.
- Failure mode is the same picture + same pads, never “Waiting for host offer.”

---

## Math (locked before code)

### Inventory

| Kind | What |
|------|------|
| Entities | Host display / PCSX2 (period `T_host`), WGC, encoder, 3 `PlayerConn`s, CLVD, RTP, governor, pad |
| Actions | Capture, encode, copy to N friends, copy to P paths, shed DC, thin-RTP skip, wake, climb `R` only |
| Measurable | `f_host` (1/s), `f_cmd` (1/s), `R` (kbps), `age` (ms), shed%, present fps, NIC kbps |
| Hard | `f_cmd = f_host`; cannot emit a frame not captured; one-way ≥ 14 ms; shed% trigger > 8 |

### Invariants

1. **fps-hold:** `f_cmd(t) = f_host` for all t after `SET_TARGET` converges. Break attempt: old `rungs_from` steps 60→30→15. That is the bug this plan deletes.
2. **Unsynchronized wait:** mean wait at one handoff is `T/2` with `T = 1000/f`. At 60, `T/2 = 8.33 ms`. **Faster fps is the snappiness**, not the enemy.
3. **Uplink (corrected):** CLVD copies the encoder rate (plus FEC only when an AU exceeds 14 kB). RTP on WebCodecs is **IDR-only**, so `U ≈ N·R + N·(8·S_idr / T_gop)`. Full dual-send `U = N·2·R` is the old tax. Every-Nth P-frame is **not** a legal `w_rtp` — see § Code-audited math.
4. **Bits/frame:** `B = R / f`. At 60@2500, mean `B ≈ 5.2 kB` (one CLVD fragment, **FEC off**). At 15@2500, mean `B ≈ 20.8 kB` (two fragments, **FEC +14 kB**). Holding 60 at the same `R` is often *cheaper* on CLVD, not more expensive.
5. **Logged `age` is not one-way glass-to-glass.** Host `age_ms = now_us − stamp_us` after the pad echo returns. `stamp_us` is taken in `push_h264`, not at WGC. `recv_ms` / `paint_ms` are on the wire and ignored. Floor is **two** one-way lights (`≈ 28 ms`), not 14.

### Dimensionless groups

- `f_cmd / f_host` — must stay **1**
- `age / T_host` — target < 3 (under ~50 ms at 60 Hz)
- `shed%` — keep < 8
- `U / U_uplink` — must stay < 1 or sheds eat fps *effective* even if `f_cmd = 60`

### Why the old 15 fps floor felt “snappy-or-smooth”

At 15, `T/2 = 33 ms` > light. Wake saved that 33. You paid smoothness. At 60, `T/2 = 8 ms` is already below the just-noticeable ~20–40 ms. **Holding 60 is the latency win.** Thin RTP is how 60 survives three WAN copies.

### Worked 3-friend numbers (`N=3`)

| Mode | `f` | `R` | `w_rtp` | `U` kbps | mean encode wait | notes |
|------|-----|-----|---------|----------|------------------|--------|
| Tonight trickle, full dual | 15 | 2500 | 1 | 15 000 | 33 ms | playable, stepped |
| PR #45 B climb, full dual | 30 | 2500 | 1 | 15 000 | 17 ms | still not host fps |
| 60 @ 10M, full dual | 60 | 10000 | 1 | **60 000** | 8 ms | sheds → death spiral |
| 60 @ 2500, full dual | 60 | 2500 | 1 | 15 000 | 8 ms | hold if uplink ≥ 15M |
| **60 @ 2500, thin N=12** | 60 | 2500 | ~0.08 + IDRs | **~8 200** | 8 ms | **this plan’s floor** |
| 60 @ 5000, thin N=12 | 60 | 5000 | ~0.08 | ~16 400 | 8 ms | climb if sheds stay < 8% |

Rescue on WebCodecs is **IDR-only** (`T_gop ≤ 1 s` from `MF_MT_MAX_KEYFRAME_SPACING = fps`, host also asks every 2 s). Unhide after a CLVD hole waits for the next independent picture, not a skipped-reference P. Safari still gets every frame (`PATH_RTP`).

---

## Code-audited math (do not implement from the first table)

Walked: `win_capture.rs` submit loop, `mf_encoder.rs`, `main.rs` pre-encoded relay + `push_to_all`, `webrtc_peer.rs` `path_flags` / `push_h264` / shed / FEC / expedite, `video_frame.rs` 14 kB + XOR, `age.rs` + `ageEcho.ts` + `player.ts` echo site, `link_gov.rs`, `signal.rs` presets, `wan3_math.rs`.

### System in plain language

A box on one desk draws pictures on a steady beat. Another program copies each picture, shrinks it into a coded packet, and hands the same packet to three far-away screens **twice** (a fast private pipe and a public pipe). When the private pipe hiccups, the public pipe is supposed to already have *some* picture ready. A thumb press on a far desk has to travel in, poke the game, wait for the next copied picture, and travel back out. The host writes down one number it calls `age` when the far desk talks back. That number is **not** the thumb-to-pixel wait, and it is **not** one trip across the ocean.

### What the first model got wrong

| First guess | Code fact | Why it matters |
|---|---|---|
| Thin RTP = every 12th P-frame | A P-frame names the previous picture. Skip 11, the 12th cannot decode. Hidden RTP then errors and asks for IDRs (`setup_video_channel` / RTCP). | Task 3 as first written **causes** the death spiral it was meant to avoid. |
| Logged `age` ≥ 14 ms | `echo_age_ms` = host now − stamp after pad echo. Stamp is `now_us()` inside `push_h264`. Echo fires in `player.ts` at **assemble**, and `paintMs` is `performance.now()` on the next line — not canvas vsync. Host ignores `recv_ms`/`paint_ms`. | `age` ≈ video one-way + assemble + pad return. Floor ≈ **28 ms** on a 14 ms path. Capture, encode, hop, decode, vsync are **outside** it. |
| 60 fps costs more bits on the wire | FEC parity is a **fixed +14 020 B** and only when `n_data > 1` (AU > 14 kB). Mean AU at 60@2500 is ~5.2 kB → one fragment → **no FEC**. Mean AU at 15@2500 is ~20.8 kB → FEC tax ~70%. | Same `R`, higher `f` can **lower** CLVD bytes/s. |
| Governor sees uplink | `on_window(shed, pushed)` only sees DC sheds / `PUSH_BUDGET`. RTP `write_sample` is invisible. Log drop% uses `dropped/(pushed+dropped)`; gov uses `dropped/pushed` (boundary mismatch at 9/100). | Stepping fps was a reaction to DC sheds, not to `U`. Thin/IDR-only RTP will not change the number the governor watches unless DC sheds fall. |
| Default ladder is 720p | `COUCHLINK_PRESET` default is **1080p60 / 18 Mbps**. Live 720@15/2500 was a 720p60 baseline after the old ladder. | `f_cmd = f_host` must hold for **whichever** preset. 1080@60@18M dual-send is `U = 108 Mbps`. |

### Inventory (measurable, with a home in the tree)

| Quantity | Unit | Where it already exists |
|---|---|---|
| `f_host` / `f_cmd` | 1/s | preset; `SET_TARGET`; overlay `target_fps` |
| `R` | kbps | `EncodeTarget.bitrate_kbps` |
| `T = 1000/f` | ms | `wan3_math::period_ms` |
| Encoder submit wait | ms | `mf_encoder.rs`: “capture→encoded tracks `1/(2f)` + encode; ~12 ms at 60” |
| Host relay wait | ms | pre-encoded cadence **2 ms** (`main.rs`) — leftover ~1 ms, not `T/2` |
| IDR period | s | MF GOP `MF_MT_MAX_KEYFRAME_SPACING = fps` (**1 s**); host `IDR_INTERVAL` **2 s** |
| CLVD fragment cap | B | `VIDEO_MAX_FRAGMENT_PAYLOAD = 14_000` |
| FEC extra | B | `26 + 2 + 14_000 = 14_028` when `n_data > 1` |
| DC shed trigger | B | `VIDEO_DC_MAX_BUFFERED = 256 KiB` |
| Push stall cap | ms | `PUSH_BUDGET = 50` (P), `1000` (IDR) |
| Pad | B, Hz | 31 B × 250 × N = **186 kbps** at N=3 |
| Logged age | ms | 64-sample ring; p50/p95; **RTT-like** |
| Unused | ms | `AgeEcho.recv_ms`, `paint_ms`; win-capture arrived/sent |

### Invariants (attacked)

1. **Predictive decode (structural, exact).** A decoder cannot show a P-frame whose reference it lacks. *Break attempt:* send P12 without P1–P11 on RTP. Fails. Surviving thin designs: **IDR-only**, or **every frame**, or a second independent encode (forbidden here).
2. **fps-hold (commanded, exact).** After `SET_TARGET` converges, `f_cmd = f_host`. Old `rungs_from` violates it (60→30→15).
3. **Cannot emit a picture not captured (exact).**
4. **One-way light (bounded).** Transit ≥ ~14 ms on the 2026-08-22 path. Logged age ≥ ~28 ms.
5. **Unsynchronized wait (approximate).** Mean wait `T/2` **only** at a handoff that is still periodic and unlocked. Host relay is already 2 ms — that `T/2` is gone. The remaining `T/2` is **win-capture `next_submit`** (`tick = 1e6/fps µs`) and the friend’s display. Wake zeros **one** `next_submit` sleep (`EXPEDITE_ONCE`).
6. **Shed → IDR (monotone coupling).** A shed non-keyframe calls `request_keyframe()`. More P-frames per second (60 vs 15) means more chances to trip this **unless** DC occupancy stays under 256 KiB. Holding 60 without thinning CLVD, or while dual-sending 10–18 Mbps, feeds the spiral `link_gov.rs` was written to stop.
7. **FEC is a step function of AU size, not of fps.** Tax = 0 if `⌈S/14000⌉ ≤ 1`, else +14 kB / AU.

### Symmetries and what they kill

- **Relabel friends.** `U` scales as `N`. `push_to_all` is concurrent (`join_all`) — sequential 50 ms × 3 is already dead.
- **Swap fps and bitrate labels.** They are independent encoder knobs (`SET_TARGET` carries both). Any ladder that steps them together is a *policy*, not physics. The old policy optimized the wrong group (`U`) by attacking the wrong variable (`f`).
- **Description symmetry of `age`.** Relabeling “age” as one-way does not survive `echo_age_ms`. Equations about felt lag must not use overlay p50 as if it were `L`.

### Dimensionless groups (pruned)

Fundamental units: time, bits, count-of-friends, count-of-pictures.

| Group | Meaning | Target |
|---|---|---|
| `φ = f_cmd / f_host` | fps-hold | **= 1** |
| `ψ = f_present / f_cmd` | silent shed | **≥ 0.9** |
| `ρ = U / U_uplink` | bit saturation | **< 1** |
| `σ` = shed% (gov) | DC only | **≤ 8** |
| `α = A / (2·light)` | age vs RTT floor | **→ 1⁺** |
| `λ = L / T_host` | felt lag in frame-times | **< 3** (~50 ms at 60) |
| `κ = S_au / 14000` | FEC on/off | **< 1** on deltas is a gift |

`age / T_host < 3` from the first draft mixed RTT-age with a one-way period. Replace with `λ` for felt lag and `α` for the overlay number.

### Uplink, worked by hand (N = 3, FEC on)

Mean payload `S = R / (8 f)` bytes (CBR fiction; IDRs steal from P-frames).

**FEC per second** ≈ `f · 14028 · 1[S > 14000]`.

| Mode | `f` | `R` | mean S | FEC/s | CLVD kbps / friend | RTP | `U` |
|---|---:|---:|---:|---:|---:|---|---:|
| Live trickle, full dual | 15 | 2500 | 20.8 kB | ~15 × 14 kB | ~4.2 M | full 2.5 M | **~20 M** not 15 M |
| 30@2500, full dual | 30 | 2500 | 10.4 kB | 0 | ~2.5 M | 2.5 M | 15 M |
| 60@2500, full dual | 60 | 2500 | 5.2 kB | 0 | ~2.5 M | 2.5 M | 15 M |
| **60@2500, IDR-only RTP** | 60 | 2500 | 5.2 kB | 0 | ~2.5 M | ~0.5 M if 60 kB / 1 s | **~9 M** |
| 60@10M, full dual | 60 | 10000 | 20.8 kB | on | ~17 M | 10 M | **~81 M** |
| 60@10M, IDR-only RTP | 60 | 10000 | 20.8 kB | on | ~17 M | ~0.5 M | **~52 M** |
| 1080p60@18M, full dual | 60 | 18000 | 37.5 kB | on | ~31 M | 18 M | **~147 M** |

The first table’s `U = 6R` **under-counts** whenever FEC is on, and **over-counts** RTP after IDR-only. 60@2500 IDR-only is the only 3-friend point that sits near a typical home uplink *and* keeps `φ = 1`.

`S_idr ≈ 60 kB` is the `link_gov` bench number, not a live histogram. Treat `w_rtp_idr = 8 S_idr / T_gop / R` as measured later (`α` does not depend on it).

### Two different delays (do not optimize the proxy)

**Felt input lag** (the product):

```
L = T_pad_fwd + T_vigem + T_emu + T_wgc
  + T_submit + T_enc + T_hop + T_relay
  + T_net_vid + T_assemble + T_decode + T_vsync
```

| Term | 15 fps, no wake | 60 fps + wake | Code |
|---|---:|---:|---|
| `T_submit` mean | 33 ms | **~0 on press**, 8 ms idle | `next_submit` + `X` |
| `T_enc` | ~0 GPU | ~0 GPU | `host_stats` encode |
| `T_relay` | ~1 ms | ~1 ms | 2 ms pre-encoded tick |
| `T_net_vid` | ≥ 14 | ≥ 14 | light |
| `T_vsync` | ~8 | ~8 | friend’s 60 Hz |
| `T_pad_fwd` | ~4–14 | same | 250 Hz CLPD |

Holding 60 is the only legal way to delete the 33 ms submit term **on idle frames**. Wake only deletes it on a press. That is why “snappy at 15” still looked stepped.

**Logged age** (the overlay):

```
A = (host_now − stamp_push) = T_net_vid + T_assemble + T_net_pad + queues
```

Missing from `A`: everything before `push_h264`, decode, vsync. Extra in `A`: pad return. Underutilized: echo `recv_ms`/`paint_ms` (today they measure JS, not paint — echo is before canvas).

**Record-low claim, restated without mixing A and L:**

- Hard: `φ = 1`, `ψ ≥ 0.9`, `σ ≤ 8`, RTP flag stays true.
- Soft sharpness: `R` may fall.
- Felt: `L` p50 below the 15 fps+wake night. Instrument is still incomplete for `L`; `A` p50 is a **lower bound on a slice**, not `L`.
- Overlay gate (same 3-friend WAN): `A_p50 < 45` is “RTT-like under ~1.6 × 28 ms”, not “one-way under 45”. If return is 14, that is ~31 ms of video+assemble — plausible at 60, impossible to read as 15 fps submit wait.

### State variables (Markov test)

`(f_cmd, R, buffered_dc, last_idr, expedite_flag, clean_windows)` is closer to Markov than `(fps, bitrate)` as one rung index.

History that still matters: GOP reference at each decoder (hidden RTP vs CLVD). If hidden RTP is IDR-only, its state is “last IDR or black”, not “N frames behind.” That is the point of IDR-only — the rescue state stays independent.

### Optimization (information structure)

**Known at decision time:** DC sheds this 5 s window, commanded `(f, R)`, present_path per friend. **Not known:** NIC bytes, RTP bytes, one-way `L`, friend’s vsync phase.

**Decision:** `R` only. **Forbidden decision:** `f`.

**Hard:** `φ = 1`, `path_flags` unchanged, no IDR-on-press, no second encoder.
**Soft:** sharpness, rescue staleness ≤ `T_gop` (1–2 s).

### Failed guesses (keep)

| Guess | Why it failed |
|---|---|
| Cut fps to save `U` | `U` is `N·(w_c+w_r)·R`, not `N·f`. You paid `T/2`. |
| Every-Nth P-frame thin RTP | Breaks invariant 1; induces IDR storm. |
| Cut `send_rtp` after first paint | `b26cf34` freeze. |
| Treat overlay age as `L` | Stamp is late; echo includes return; paint isn’t paint. |
| `U = 6R` | Ignores FEC step and IDR-only RTP. |
| Capacity is encoder-fps / N / paths | Live 15 fps hold already refuted this (`wan3_math`). |

### Domain of validity

- Fails if uplink < ~9 Mbps at 720@60@2500 IDR-only — then drop **`R`**, not `f`.
- Fails if someone runs default 1080p60@18M and expects 3-friend WAN to hold — `ρ ≫ 1` even IDR-only.
- Fails if `COUCHLINK_RTP_EVERY_N=1` (full dual) and uplink < 15 Mbps at 2500.
- Rescue picture may be up to one GOP stale. CLVD is the present path.
- Does not beat light. Does not put capture/encode into `A`.
- `PATH_RTP` (Safari) is full dual-send for that seat — one Safari friend adds a full `R` of RTP.

### Task 1 / 3 corrections (implementers)

- `host_uplink_thin_kbps` is **not** `n * R * (1 + 1/12)`. Use `n * R + n * idr_kbps` with `idr_kbps = (8 * S_idr / T_gop) / 1000`, `S_idr` from the 60 kB bench until live-measured, `T_gop = 1`.
- `should_send_rtp(keyframe, path)` = `keyframe || path == PATH_RTP || env full-dual`.
- Default is IDR-only on WebCodecs, not N=12.

### Objective

Minimize felt `L` (instrument still a slice). Overlay `age_p50` is a **RTT-like proxy**, floor ≈ 28 ms, not 14. Do not treat it as `L`. Constraints:

- `f_present ≥ 0.9 × f_host` on each of 3 browsers (hard)
- `shed% ≤ 8` (hard)
- `path_flags` unchanged (hard)
- `R` free (soft — sharpness)

Target on the same 3-friend WAN as 2026-08-22: **`age_p50 < 45 ms`**, **`age_p95 < 80 ms`**, **present fps ≥ 55** at 720p. That is lower age *and* higher fps than the 15/2500 night.

### Domain of validity

- Fails if home uplink < ~9 Mbps; then 60@2500 IDR-only still sheds — drop `R`, **not** `f`.
- Fails if full dual-send and uplink < 15 Mbps at 2500.
- Does not beat 14 ms of light. Logged age does not beat ~28 ms.
- Rescue may be up to one GOP stale (1–2 s). CLVD is the present path.

---

## File Structure

**Created**

- Tests in `crates/host/src/wan3_math.rs` (fps-hold + IDR-only `U`)
- Tests in `crates/host/src/link_gov.rs` (no fps step)
- Tests in `crates/host/src/webrtc_peer.rs` (RTP sent on IDR only for WebCodecs; every frame for Safari)

**Modified**

- `crates/host/src/link_gov.rs` — `rungs_from` keeps `fps: baseline.fps` on every rung; only `bitrate_kbps` halves
- `crates/host/src/webrtc_peer.rs` — `push_h264`: skip `write_sample` for WebCodecs P-frames unless full-dual
- `crates/host/src/wan3_math.rs` — lock the new ladder + `host_uplink_idr_only_kbps`
- `crates/proto/src/signal.rs` / overlay optional: show `f_cmd` (already `target_fps`)

**Not created**

- A second encoder. SVC. Shared memory. Cutting `send_rtp`.

---

### Task 1: Math probes — fps-hold and thin uplink

**Files:**
- Modify: `crates/host/src/wan3_math.rs`
- Test: same file

**Interfaces:**
- Produces: `host_uplink_idr_only_kbps(enc_kbps, n, idr_bytes, gop_s) -> u32`  
  `// U = n * R + n * (8 * idr_bytes / gop_s / 1000)`
- Consumes: existing `N_FRIENDS`, `LIVE_TRICKLE`, `P720`, bench `IDR_BYTES = 60_000`, `T_gop = 1`

- [ ] **Step 1: Write the failing tests**

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
    // 3*2500 + 3*(480) = 8940
    assert!(thin < 9_500 && thin > 8_000, "idr-only {thin}");
    assert!(thin < full);
}
```

- [ ] **Step 2: Run** `cargo test -p couchlink-host --bins -- every_rung_from_p720_holds_60_fps idr_only_rtp -- --nocapture`  
      Expected: FAIL (`rungs_from` still has 30/15).

- [ ] **Step 3: Add `host_uplink_idr_only_kbps` only (do not change `rungs_from` yet).** Make `idr_only_rtp` PASS; leave `every_rung` FAIL until Task 2.

- [ ] **Step 4: Commit** `test(math): fps-hold and IDR-only RTP uplink probes`

---

### Task 2: Governor steps bitrate only

**Files:**
- Modify: `crates/host/src/link_gov.rs` `rungs_from`
- Modify: `crates/host/src/wan3_math.rs` `rungs_from` copy (must match)
- Test: `link_gov.rs` `persistent_shed_steps_down_to_trickle` — **change assertion**: floor fps is still 60, bitrate is baseline/8 or /4

**Interfaces:**
- `rungs_from(baseline)` → `[baseline, R/2 @ same fps, R/4 @ same fps, R/8 @ same fps]` with `fps` copied from baseline. Dedup if already at floor.
- Example P720: `60/10000, 60/5000, 60/2500, 60/1250`

- [ ] **Step 1: Write**

```rust
#[test]
fn persistent_shed_never_drops_fps() {
    let mut gov = LinkGov::new(P720);
    for _ in 0..20 {
        gov.on_window(40, 100);
        assert_eq!(gov.current().fps, 60);
    }
    assert!(gov.current().bitrate_kbps <= 2_500);
}
```

- [ ] **Step 2: Run** — FAIL on current 15 fps floor.

- [ ] **Step 3: Implement `rungs_from`.** Update `wan3_math` copy and any test that expected 15/2500 as fps floor (`live_trickle_is_the_p720_ladder_floor` becomes “bitrate floor 1250 or 2500, fps 60”).

- [ ] **Step 4: Run** `cargo test -p couchlink-host --bins -- link_gov wan3_math`  
      Expected: PASS. `every_rung_from_p720_holds_60_fps` now PASS.

- [ ] **Step 5: Commit** `feat(gov): hold host fps; step bitrate only`

---

### Task 3: IDR-only RTP rescue (not every-Nth P)

**Files:**
- Modify: `crates/host/src/webrtc_peer.rs` `push_h264`
- Test: `webrtc_peer.rs` `controller_host_tests`

**Interfaces:**
- `fn rtp_full_dual() -> bool` — `COUCHLINK_RTP_EVERY_N=1` or `COUCHLINK_RTP_FULL=1` restores every frame
- `fn should_send_rtp(keyframe: bool, path: u8, full_dual: bool) -> bool` — `full_dual || path == PATH_RTP || keyframe`
- `write_sample` only when `send_rtp && should_send_rtp(...)`
- CLVD path unchanged. `path_flags` unchanged. **Never** send a P-frame on RTP unless every preceding reference since the last IDR was also sent.

- [ ] **Step 1: Write**

```rust
#[test]
fn should_send_rtp_idr_only_on_webcodecs_full_on_safari() {
    assert!(should_send_rtp(true, PATH_WEBCODECS, false));
    assert!(!should_send_rtp(false, PATH_WEBCODECS, false));
    assert!(should_send_rtp(false, PATH_RTP, false));
    assert!(should_send_rtp(false, PATH_WEBCODECS, true));
}

#[test]
fn webcodecs_path_keeps_rtp_flag_even_when_thin() {
    assert_eq!(path_flags(PATH_WEBCODECS), (true, true));
}
```

- [ ] **Step 2: Run** — first test FAIL (fn missing).

- [ ] **Step 3: Implement `should_send_rtp` + gate `write_sample`.**

- [ ] **Step 4: Run** `cargo test -p couchlink-host --bins -- should_send_rtp webcodecs_path`  
      Expected: PASS.

- [ ] **Step 5: Commit** `feat(rtp): IDR-only rescue on WebCodecs — never skip-reference P-frames`

---

### Task 4: Live 3-friend 60 Hz gate

**Files:** none required if Tasks 1–3 green. Record numbers in the Task 4 commit message.

- [ ] **Step 1:** Rebuild host + win-capture. `cd web && npm run build`. Friends hard-refresh. Do **not** kill `ds-vhid` / PCSX2.

- [ ] **Step 2:** 3 minutes, 3 browsers, mix of pads/KBM. Overlay must show `target_fps = 60` (or host preset fps) the whole time.

- [ ] **Step 3: Pass**
  - present fps ≥ 55 on each friend (or ≥ 0.9 × preset)
  - `age_p50 < 45` ms, `age_p95 < 80` ms
  - shed% not a new sustained > 8
  - zero `chunk too short`
  - pad Hz ≥ 100
  - a CLVD stall still unhides via RTP (next IDR) within ~1–2 s — not a frozen last frame forever

- [ ] **Step 4: Fail and stop** if present fps collapses to ~15 while `target_fps` says 60 — that is silent shedding, not a governor fps cut. Fix `R` or DC buffer, do not reintroduce 15 fps rungs.

- [ ] **Step 5: Commit** `chore(lag): 3-friend 60fps age p50=… p95=…` with the overlay snippet. No merge if Step 3 fails.

---

## Risk register

| ID | Risk | Mitigation |
|----|------|------------|
| R1 | Thin RTP too thin → Safari / rescue black | Safari (`PATH_RTP`) gets every frame; WebCodecs keeps CLVD at 60; RTP gets every IDR |
| R2 | 60@2500 looks like mush | Climb `R` after 8 clean windows; fps stays 60 |
| R3 | Full dual-send sneaks back | Default IDR-only; test `idr_only_rtp_is_under_full_dual` |
| R4 | Someone “fixes” lag by cutting fps again | `every_rung_from_p720_holds_60_fps` is a merge gate |
| R5 | Cutting `send_rtp` to save bits | Forbidden. Same freeze as `b26cf34` |
| R6 | Every-Nth P-frame “thin” RTP | Illegal. Hidden decoder loses references and storms IDRs |

## Hard no's

- Step fps below `f_host` in `rungs_from`
- `path_flags` → `(false, true)` after first paint
- IDR-on-press
- Software x264 as a 60 fps experiment
- Kill `ds-vhid` to test

Plan complete. Implementation is **not** in this file.
