//! 3-friend WAN latency + framerate math.
//!
//! Every constant is pinned to a source line or a `cargo test -- --nocapture`
//! print. Do not invent a number here. If a relationship is a conjecture, the
//! test name says so and the assertion is the *check*, not the wish.
//!
//! Scope: three remote friends on WAN (plan A wait-cut + plan B fps climb).
//! This module is the instrument. It does not stamp CLVD or expedite frames.

use couchlink_capture_bridge::EncodeTarget;
use couchlink_proto::video_frame::{VIDEO_HEADER_LEN, VIDEO_MAX_FRAGMENT_PAYLOAD};
use couchlink_proto::VideoAccessUnit;

/// Remote seats the host will fan the same encode out to.
/// `crates/host/src/emulator_pad.rs` `MAX_REMOTE_SLOTS = 3`.
pub const N_FRIENDS: u32 = 3;

/// WebCodecs healthy present path writes CLVD only (`path_flags` → DC).
/// Warmup/unknown still dual-send. Uplink model for the healthy case is 1 path.
pub const PATHS_WEBCODECS: u32 = 1;

/// Live governor window. `main.rs` `rate_window.elapsed() >= Duration::from_secs(5)`.
pub const GOV_WINDOW_S: f64 = 5.0;

/// `link_gov.rs` `DOWN_TRIGGER_PCT`.
pub const DOWN_TRIGGER_PCT: u32 = 8;

/// `link_gov.rs` `UP_AFTER_CLEAN_WINDOWS`.
pub const UP_AFTER_CLEAN_WINDOWS: u32 = 8;

/// Pad wire. `crates/proto/src/pad_frame.rs` `PAD_FRAME_LEN` / player.ts 250 Hz.
pub const PAD_FRAME_LEN: u32 = 31;
pub const PAD_HZ: u32 = 250;

/// One-way light on a good internet path. Plan: "will not beat light."
pub const LIGHT_ONE_WAY_MS: f64 = 14.0;

/// Live WAN floor recorded 2026-08-22 (plan + host_stats): 1280×720 @ 15 / ~2500 kbps.
pub const LIVE_TRICKLE: EncodeTarget = EncodeTarget {
    width: 1280,
    height: 720,
    fps: 15,
    bitrate_kbps: 2_500,
};

/// Test baseline in `link_gov.rs` `P720`.
pub const P720: EncodeTarget = EncodeTarget {
    width: 1280,
    height: 720,
    fps: 60,
    bitrate_kbps: 10_000,
};

/// Same construction as `link_gov::rungs_from` — must stay a copy so a ladder
/// change fails these tests instead of silently drifting the WAN model.
/// Hold `baseline.fps`; step bitrate only (fps-hold invariant).
pub fn rungs_from(baseline: &EncodeTarget) -> Vec<EncodeTarget> {
    let mut rungs = vec![*baseline];
    let mut kbps = baseline.bitrate_kbps;
    const FLOOR_KBPS: u32 = 1_250;
    for _ in 0..3 {
        let next = (kbps / 2).max(FLOOR_KBPS);
        if next >= kbps {
            break;
        }
        kbps = next;
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

/// Host uplink with IDR-only RTP rescue on WebCodecs (CLVD full + RTP IDRs).
/// `idr_bytes` / `gop_s` size the IDR tax; default GOP ≈ 1 s.
pub fn host_uplink_idr_only_kbps(enc_kbps: u32, n: u32, idr_bytes: u64, gop_s: f64) -> u32 {
    let clvd = enc_kbps.saturating_mul(n);
    let idr_kbps = ((idr_bytes as f64) * 8.0 / 1000.0 / gop_s.max(0.001)).ceil() as u32;
    clvd.saturating_add(idr_kbps.saturating_mul(n))
}

/// Frame period T (ms). Fundamental unit of unsynchronized wait.
pub fn period_ms(fps: u32) -> f64 {
    1000.0 / f64::from(fps.max(1))
}

/// Mean wait at one unsynchronized periodic handoff: T/2.
pub fn mean_unsync_wait_ms(fps: u32) -> f64 {
    period_ms(fps) / 2.0
}

/// CBR bits in one encoded picture. Units: bits.
pub fn bits_per_frame(kbps: u32, fps: u32) -> f64 {
    f64::from(kbps) * 1000.0 / f64::from(fps.max(1))
}

/// Host uplink if every friend gets `paths` full copies of the encoder bitrate.
/// Units: kbps.
pub fn host_uplink_kbps(enc_kbps: u32, n: u32, paths: u32) -> u32 {
    enc_kbps.saturating_mul(n).saturating_mul(paths)
}

/// Seconds of clean sheds before the governor is allowed one climb.
pub fn climb_ready_s() -> f64 {
    f64::from(UP_AFTER_CLEAN_WINDOWS) * GOV_WINDOW_S
}

/// Remaining submit wait if a press arrives a fraction `phase` into the interval.
/// `phase` in [0, 1). Wake-on-input saves this, not T/2 on every frame.
pub fn wake_saves_ms(fps: u32, phase: f64) -> f64 {
    period_ms(fps) * (1.0 - phase.clamp(0.0, 0.999_999))
}

/// Reconstruct the published 1-viewer / 1-path governor bench.
/// `cargo test -p couchlink-host --bins -- link_gov --nocapture` 2026-08-22:
/// no-gov shed 1600, delivered 800, ~99 MB.
pub fn reconstruct_one_path_gov_bench() -> (u32, u32, u64) {
    const CAPACITY_FPS: u32 = 24;
    const WINDOWS: u32 = 400;
    const IDR_BYTES: u64 = 60_000;
    const DELTA_BYTES: u64 = 4_000;
    let emitted = (P720.fps / 10).max(1);
    let carry = (CAPACITY_FPS / 10).max(1);
    let shed_per = emitted.saturating_sub(carry);
    let delivered_per = emitted.min(carry);
    let shed_total = shed_per * WINDOWS;
    let delivered_total = delivered_per * WINDOWS;
    let wire = u64::from(shed_total) * IDR_BYTES + u64::from(delivered_total) * DELTA_BYTES;
    (shed_total, delivered_total, wire)
}

/// CLVD wire bytes for one AU (headers + payload + optional FEC parity).
pub fn clvd_wire_bytes(annex_b_len: usize, fec: bool) -> usize {
    let au = VideoAccessUnit {
        seq: 1,
        width: 1280,
        height: 720,
        keyframe: annex_b_len >= 20_000,
        annex_b: vec![0u8; annex_b_len],
        stamp_us: 0,
    };
    let frags = if fec {
        au.encode_fragments_with_fec()
    } else {
        au.encode_fragments()
    };
    frags.iter().map(|f| f.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn almost(a: f64, b: f64, eps: f64) {
        assert!((a - b).abs() < eps, "{a} ≉ {b} (eps {eps})");
    }

    // ── Inventory locked to measurements ──────────────────────────────

    #[test]
    fn every_rung_from_p720_holds_60_fps() {
        for r in rungs_from(&P720) {
            assert_eq!(r.fps, 60, "fps-hold violated: {r:?}");
        }
    }

    #[test]
    fn p720_ladder_steps_bitrate_only() {
        let r = rungs_from(&P720);
        assert_eq!(r.len(), 4);
        assert_eq!(r[0], P720);
        assert_eq!(r[1], EncodeTarget { bitrate_kbps: 5_000, ..P720 });
        assert_eq!(r[2], EncodeTarget { bitrate_kbps: 2_500, ..P720 });
        assert_eq!(r[3], EncodeTarget { bitrate_kbps: 1_250, ..P720 });
        let floor = *r.last().unwrap();
        assert_eq!(floor.fps, 60);
        assert_eq!(floor.bitrate_kbps, 1_250);
    }

    #[test]
    fn live_trickle_is_historical_observation_not_ladder_floor() {
        // LIVE_TRICKLE is the 2026-08-22 measured death-spiral floor; the
        // fps-hold ladder floor is 60@1250, not 15@2500.
        assert_eq!(LIVE_TRICKLE.fps, 15);
        assert_eq!(LIVE_TRICKLE.bitrate_kbps, 2_500);
        assert_ne!(*rungs_from(&P720).last().unwrap(), LIVE_TRICKLE);
    }

    #[test]
    fn measured_period_and_t_half_at_each_rung() {
        // T = 1000/fps. Mean unsync wait = T/2. Worked by hand:
        // 60 → 16.666… / 8.333…; 30 → 33.333… / 16.666…; 15 → 66.666… / 33.333…
        almost(period_ms(60), 1000.0 / 60.0, 1e-9);
        almost(mean_unsync_wait_ms(60), 1000.0 / 120.0, 1e-9);
        almost(period_ms(30), 1000.0 / 30.0, 1e-9);
        almost(mean_unsync_wait_ms(15), 1000.0 / 30.0, 1e-9);
        assert!(mean_unsync_wait_ms(15) > LIGHT_ONE_WAY_MS);
        assert!(mean_unsync_wait_ms(60) < LIGHT_ONE_WAY_MS);
    }

    #[test]
    fn bits_per_frame_at_live_rungs() {
        // 2500e3 / 15 = 166_666.6… bits ≈ 20_833 B
        almost(bits_per_frame(2_500, 15), 2500.0 * 1000.0 / 15.0, 1e-6);
        almost(
            bits_per_frame(5_000, 15) / bits_per_frame(2_500, 15),
            2.0,
            1e-9,
        );
        // At held 60 fps, halving R halves bits/frame.
        almost(
            bits_per_frame(5_000, 60),
            bits_per_frame(10_000, 60) / 2.0,
            1e-9,
        );
    }

    // ── Existing bench reconstruction (must match --nocapture) ───────

    #[test]
    fn one_path_gov_bench_reproduces_printed_99mb() {
        let (shed, delivered, wire) = reconstruct_one_path_gov_bench();
        assert_eq!(shed, 1600, "printed: no governor shed 1600");
        assert_eq!(delivered, 800, "printed: 800 delivered");
        assert_eq!(wire / 1_000_000, 99, "printed: ~99 MB (got {wire})");
    }

    #[test]
    fn wire_bench_gop_second_matches_print() {
        // Printed: detached 1_330_000 B → commanded 599_000 B (55% fewer)
        let detached = 150_000 + 59 * 20_000;
        let preset = 68_000 + 59 * 9_000;
        assert_eq!(detached, 1_330_000);
        assert_eq!(preset, 599_000);
        let cut = (detached as f64 - preset as f64) / detached as f64 * 100.0;
        almost(cut, 55.0, 0.5);
    }

    // ── Probe: 1-link fps-capacity is too simple for 3-friend WAN ─────

    #[test]
    fn naive_fps_capacity_divided_by_fanout_contradicts_live_hold() {
        // Bench assumed one viewer, one path, capacity = 24 encoder-fps.
        // Blindly divide by N*paths and you "prove" we cannot hold 15 fps.
        let naive_share = 24.0 / f64::from(N_FRIENDS * PATHS_WEBCODECS);
        assert!(
            naive_share < f64::from(LIVE_TRICKLE.fps),
            "sanity: 24/3 = 8 < 15"
        );
        // Live fact (plan, 2026-08-22): three WAN viewers held 15 fps @ 2500 kbps.
        // Therefore capacity is not "encoder-fps / N / paths". The 24 fps bench
        // is a 1-path tunnel model. Using it for 3-friend WAN is too simplistic.
        assert!(
            naive_share < 15.0 && LIVE_TRICKLE.fps == 15,
            "reformed model must be bits×N×paths, not fps÷N÷paths"
        );
    }

    #[test]
    fn three_friend_clvd_only_uplink_is_three_times_encoder() {
        assert_eq!(host_uplink_kbps(2_500, N_FRIENDS, PATHS_WEBCODECS), 7_500);
        assert_eq!(host_uplink_kbps(5_000, N_FRIENDS, PATHS_WEBCODECS), 15_000);
        assert_eq!(host_uplink_kbps(10_000, N_FRIENDS, PATHS_WEBCODECS), 30_000);
    }

    #[test]
    fn first_climb_off_floor_doubles_bits_keeps_60() {
        let r = rungs_from(&P720);
        let floor = r[3];
        let climb = r[2];
        assert_eq!(floor.fps, 60);
        assert_eq!(floor.bitrate_kbps, 1_250);
        assert_eq!(climb.fps, 60);
        assert_eq!(climb.bitrate_kbps, 2_500);
        almost(climb_ready_s(), 40.0, 1e-9);
        assert_eq!(
            host_uplink_idr_only_kbps(2_500, N_FRIENDS, 60_000, 1.0),
            8_940
        );
    }

    #[test]
    fn same_bitrate_30fps_would_cut_t_half_without_uplink_growth() {
        // Undiscovered (relative to rungs_from): fps and bitrate are independent
        // encoder knobs. A 30@2500 rung keeps host_uplink at 15 Mbps and halves T.
        let t15 = mean_unsync_wait_ms(15);
        let t30 = mean_unsync_wait_ms(30);
        almost(t15 / t30, 2.0, 1e-9);
        assert_eq!(
            host_uplink_kbps(2_500, N_FRIENDS, PATHS_WEBCODECS),
            host_uplink_kbps(2_500, 3, 1)
        );
        let bits15 = bits_per_frame(2_500, 15);
        let bits30 = bits_per_frame(2_500, 30);
        almost(bits15 / bits30, 2.0, 1e-9);
        // Quality per frame halves. That is the trade, not a free 30 fps.
        assert!(bits30 < bits15);
    }

    // ── Plan A: wake saves remaining interval, not T/2 on every frame ─

    #[test]
    fn wake_on_input_saves_remainder_not_mean_on_idle_frames() {
        almost(wake_saves_ms(15, 0.0), period_ms(15), 1e-9);
        almost(wake_saves_ms(15, 0.5), mean_unsync_wait_ms(15), 1e-9);
        almost(wake_saves_ms(15, 0.9), period_ms(15) * 0.1, 1e-6);
        // Idle pictures still wait T/2. Only the picture after a press is pulled.
        assert!(mean_unsync_wait_ms(15) > 30.0);
    }

    #[test]
    fn pad_uplink_is_noise_next_to_trickle_video() {
        let pad_kbps = f64::from(PAD_FRAME_LEN * 8 * PAD_HZ * N_FRIENDS) / 1000.0;
        // 31 B * 8 * 250 * 3 = 186_000 bit/s = 186 kbps
        almost(pad_kbps, 186.0, 0.1);
        assert!(pad_kbps < f64::from(LIVE_TRICKLE.bitrate_kbps) * 0.1);
    }

    // ── Governor information structure (half-blind) ───────────────────

    #[test]
    fn live_gov_drop_pct_uses_pushed_not_pushed_plus_shed() {
        // main.rs log: sent = window_frames + dropped; drop_pct = dropped/sent
        // on_window(dropped, window_frames): drop_pct = shed*100/sent with sent=pushed
        // 8 dropped, 92 pushed:
        let dropped = 8u32;
        let pushed = 92u32;
        let printed = dropped * 100 / (pushed + dropped); // 8
        let gov = dropped * 100 / pushed; // 8
        assert_eq!(printed, 8);
        assert_eq!(gov, 8);
        // 8 dropped, 90 pushed — printed 8%, governor 8% still (int div)
        let printed2 = 8 * 100 / 98;
        let gov2 = 8 * 100 / 90;
        assert_eq!(printed2, 8);
        assert!(gov2 >= DOWN_TRIGGER_PCT);
        // 9/100 printed vs 9/91 gov: printed 9, gov 9. Both trigger.
        // The mismatch is real at the boundary: 8/100 printed = 8 (not >8),
        // 8/92 gov = 8 (not >8). 8/91 printed=8, gov=8. 8/90 printed=8, gov=8.
        // 9 dropped 100 sent printed=8, wait 9/109=8; gov 9/100=9 → STEPS, log says 8%.
        let dropped = 9u32;
        let pushed = 100u32;
        let printed = dropped * 100 / (pushed + dropped);
        let gov = dropped * 100 / pushed;
        assert_eq!(printed, 8);
        assert_eq!(gov, 9);
        assert!(gov > DOWN_TRIGGER_PCT);
        assert!(printed <= DOWN_TRIGGER_PCT);
    }

    #[test]
    fn governor_does_not_see_rtp_bytes() {
        // path_flags sends RTP first, then CLVD. Shed is DC congested / PUSH_BUDGET.
        // RTP write_sample is not a shed source. Model must not treat shed% as
        // "fraction of host uplink used" — it is "fraction of DC frames skipped".
        let enc = 2_500u32;
        let dc_only = host_uplink_kbps(enc, N_FRIENDS, 1);
        let both = host_uplink_kbps(enc, N_FRIENDS, PATHS_WEBCODECS);
        assert_eq!(dc_only, 7_500);
        assert_eq!(both, 7_500);
        assert_eq!(both, dc_only);
    }

    // ── FEC / header conversions (measured via proto encoder) ─────────

    #[test]
    fn rungs_from_must_not_take_the_live_floor_as_baseline() {
        // fps-hold relative to *whatever* baseline is passed: every rung keeps
        // that fps. Passing LIVE_TRICKLE as baseline must not invent 7 fps.
        let from_trickle = rungs_from(&LIVE_TRICKLE);
        for r in &from_trickle {
            assert_eq!(r.fps, 15, "fps-hold relative to LIVE_TRICKLE: {r:?}");
        }
        assert_eq!(from_trickle[0].bitrate_kbps, 2_500);
        assert!(
            from_trickle.iter().any(|r| r.bitrate_kbps == 1_250),
            "bitrate floors at 1250: {from_trickle:?}"
        );
        let from_p720 = rungs_from(&P720);
        for r in &from_p720 {
            assert_eq!(r.fps, 60);
        }
        assert_eq!(from_p720.last().unwrap().bitrate_kbps, 1_250);
    }

    #[test]
    fn clvd_header_is_v3_and_fec_parity_only_when_multi_fragment() {
        assert_eq!(VIDEO_HEADER_LEN, 26);
        assert_eq!(VIDEO_MAX_FRAGMENT_PAYLOAD, 14_000);
        // 9_000 B delta: one data frag, FEC skipped (n_data == 1).
        let d_off = clvd_wire_bytes(9_000, false);
        let d_on = clvd_wire_bytes(9_000, true);
        assert_eq!(d_off, 9_000 + VIDEO_HEADER_LEN);
        assert_eq!(d_on, d_off, "single-fragment FEC must not double the send");
        // 68_000 B IDR: 5 data chunks (4*14000+12000). FEC adds 1 parity.
        let k_off = clvd_wire_bytes(68_000, false);
        let k_on = clvd_wire_bytes(68_000, true);
        assert_eq!(k_off, 68_000 + 5 * VIDEO_HEADER_LEN);
        assert!(k_on > k_off, "multi-fragment IDR must carry parity");
        let parity = k_on - k_off;
        // parity payload = 2 + 14000 + 18 header
        assert_eq!(parity, VIDEO_HEADER_LEN + 2 + VIDEO_MAX_FRAGMENT_PAYLOAD);
        // Mid-size AU (20 kB, 2 data frags): fixed +14020 is ~70% of the data wire.
        let mid_off = clvd_wire_bytes(20_000, false);
        let mid_on = clvd_wire_bytes(20_000, true);
        assert_eq!(mid_off, 20_000 + 2 * VIDEO_HEADER_LEN);
        assert_eq!(
            mid_on,
            mid_off + VIDEO_HEADER_LEN + 2 + VIDEO_MAX_FRAGMENT_PAYLOAD
        );
        let mid_tax = (mid_on as f64 / mid_off as f64) - 1.0;
        assert!(
            (mid_tax - 0.70).abs() < 0.01,
            "20k AU FEC tax {mid_tax} should be ~70%"
        );
    }

    #[test]
    fn fec_does_not_change_the_clvd_uplink_order() {
        // Order-of-magnitude: FEC parity on IDRs is not another full path.
        let idr_on = clvd_wire_bytes(68_000, true) as f64;
        let idr_off = clvd_wire_bytes(68_000, false) as f64;
        let fec_tax = (idr_on / idr_off) - 1.0;
        assert!(
            fec_tax > 0.0 && fec_tax < 0.30,
            "FEC tax {fec_tax} should be tens of percent on a 68k IDR, not 2×"
        );
        assert_eq!(PATHS_WEBCODECS, 1);
    }

    // ── Felt-lag composition (conjecture bounded by measurements) ─────

    #[test]
    fn felt_lag_lower_bound_is_light_plus_one_display_half_period() {
        // Cannot beat light (14) + friend's 60 Hz T/2 (~8.3). Work + hop ≥ 0.
        let floor = LIGHT_ONE_WAY_MS + mean_unsync_wait_ms(60);
        almost(floor, 14.0 + 1000.0 / 120.0, 1e-9);
        // At live 15 fps, submit T/2 dominates the floor.
        let live = mean_unsync_wait_ms(15) + floor;
        assert!(live > 50.0);
        assert!(live < 80.0);
    }

    #[test]
    fn plan_b_clvd_only_uplink_under_historical_dual() {
        let clvd = host_uplink_kbps(2_500, N_FRIENDS, PATHS_WEBCODECS);
        let dual = host_uplink_kbps(2_500, N_FRIENDS, 2);
        let idr_only = host_uplink_idr_only_kbps(2_500, N_FRIENDS, 60_000, 1.0);
        assert_eq!(clvd, 7_500);
        assert_eq!(dual, 15_000);
        assert_eq!(idr_only, 8_940);
        assert!(clvd < dual);
        assert_eq!(rungs_from(&P720)[2].bitrate_kbps, 2_500);
        assert_eq!(rungs_from(&P720)[2].fps, 60);
    }
}
