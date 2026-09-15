//! Ricardo beat scorecard — hard bars, not soft floors.
//!
//! Soft floors (push≥50, shed≤8) let a mediocre session "pass" while feeling
//! worse than Ricardo. These gates require matching or beating the frozen
//! playable night on every axis.

use crate::ricardo_playable_ab::ricardo_playable_a as A;

pub use crate::ricardo_playable_ab::ricardo_playable_a as RICARDO;

#[derive(Debug, Clone, Copy)]
pub struct SessionMetrics {
    pub push_fps: f64,
    pub shed_pct: u32,
    pub encoder_kbps: u32,
    pub paint_fps: f64,
    pub input_s_p50_ms: f64,
}

/// Soft scrape floors (legacy / early join). Prefer [`beats_ricardo`].
pub fn beats_ricardo_soft(m: SessionMetrics) -> bool {
    m.push_fps >= A::MIN_PUSH_FPS
        && m.shed_pct <= A::MAX_SHED_PCT
        && m.encoder_kbps >= A::ENCODER_KBPS
        && m.paint_fps >= 70.0
        && m.input_s_p50_ms <= 45.0
}

/// Hard gate: match or beat Ricardo's frozen playable night.
///
/// | Axis   | Ricardo | Required        |
/// |--------|---------|-----------------|
/// | push   | 77.8    | ≥ 74            |
/// | shed   | 0%      | ≤ 3%            |
/// | encode | 10 Mbps | ≥ A::ENCODER_KBPS |
/// | paint  | 74      | ≥ 74            |
/// | S_p50  | (n/a)   | ≤ 45 ms (wow)   |
pub fn beats_ricardo(m: SessionMetrics) -> bool {
    m.push_fps >= A::PAINT_FPS
        && m.shed_pct <= 3
        && m.encoder_kbps >= A::ENCODER_KBPS
        && m.paint_fps >= A::PAINT_FPS
        && m.input_s_p50_ms <= 45.0
}

/// Strict beat: same as hard, plus input surplus clears stretch (≤30ms).
pub fn strictly_beats_ricardo(m: SessionMetrics) -> bool {
    beats_ricardo(m) && m.input_s_p50_ms <= 30.0
}

/// Frozen self baseline from the first honest LIVE Ricardo PASS on this branch
/// (2026-08-23 probe10 / probe-ci): push 74.8 · paint 84 · S_p50 7.4 · 0% shed.
///
/// Encoder bitrate tracks `A::ENCODER_KBPS` rather than a frozen literal — the
/// production preset moved 5->10 Mbps after this probe (343ce8e, "production
/// motion — 10Mbps@60"), and a scorecard frozen at the old bitrate can never
/// clear the current Ricardo gate on that axis, which made every downstream
/// beats_ricardo/beats_self assertion fail for a reason that has nothing to do
/// with push/paint/latency regressing.
pub mod self_baseline {
    use super::A;
    pub const PUSH_FPS: f64 = 74.8;
    pub const PAINT_FPS: f64 = 84.0;
    pub const SURPLUS_P50_MS: f64 = 7.4;
    pub const ENCODER_KBPS: u32 = A::ENCODER_KBPS;
    pub const SHED_PCT: u32 = 0;
}

/// Beat-self bars — clear margin over the frozen self baseline, not a skim.
pub mod self_beat {
    use super::A;
    pub const MIN_PUSH_FPS: f64 = 90.0;
    pub const MIN_PAINT_FPS: f64 = 100.0;
    pub const MAX_SURPLUS_P50_MS: f64 = 5.0;
    pub const MIN_ENCODER_KBPS: u32 = A::ENCODER_KBPS;
    pub const MAX_SHED_PCT: u32 = 1;
}

/// Must clear Ricardo **and** the beat-self margin over our own locked scorecard.
pub fn beats_self(m: SessionMetrics) -> bool {
    beats_ricardo(m)
        && m.push_fps >= self_beat::MIN_PUSH_FPS
        && m.shed_pct <= self_beat::MAX_SHED_PCT
        && m.encoder_kbps >= self_beat::MIN_ENCODER_KBPS
        && m.paint_fps >= self_beat::MIN_PAINT_FPS
        && m.input_s_p50_ms <= self_beat::MAX_SURPLUS_P50_MS
}

pub fn beats_ricardo_push_and_paint(m: SessionMetrics) -> bool {
    m.push_fps >= A::PUSH_FPS * 0.9 && m.paint_fps >= A::PAINT_FPS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::latency_live_sim::governor::single_blip_holds_baseline;
    use crate::latency_live_sim::paint::{simulate_paint_fps, PaintSimConfig};
    use crate::latency_live_sim::simulate_two_peer_shed_counting;
    use crate::webrtc_peer::{governor_shed_counts, PushFate};

    #[test]
    fn ricardo_baseline_beats_itself() {
        let m = SessionMetrics {
            push_fps: A::PUSH_FPS,
            shed_pct: A::SHED_PCT,
            encoder_kbps: A::ENCODER_KBPS,
            paint_fps: A::PAINT_FPS,
            input_s_p50_ms: 40.0,
        };
        assert!(beats_ricardo(m));
    }

    #[test]
    fn soft_floor_session_fails_hard_gate() {
        // Old soft gate would pass this; hard gate must reject — 50fps is not Ricardo.
        let m = SessionMetrics {
            push_fps: 50.0,
            shed_pct: 8,
            encoder_kbps: A::ENCODER_KBPS,
            paint_fps: 70.0,
            input_s_p50_ms: 40.0,
        };
        assert!(beats_ricardo_soft(m));
        assert!(!beats_ricardo(m));
    }

    #[test]
    fn death_spiral_session_fails_gate() {
        let m = SessionMetrics {
            push_fps: 0.8,
            shed_pct: 50,
            encoder_kbps: 1_250,
            paint_fps: 1.0,
            input_s_p50_ms: 40.0,
        };
        assert!(!beats_ricardo(m));
    }

    #[test]
    fn target_session_passes_all_three_axes() {
        let m = SessionMetrics {
            push_fps: 78.0,
            shed_pct: 0,
            encoder_kbps: A::ENCODER_KBPS,
            paint_fps: 74.0,
            input_s_p50_ms: 35.0,
        };
        assert!(beats_ricardo(m));
    }

    #[test]
    fn joel_optimized_path_composes_to_beat_ricardo() {
        // Compose the shipped optimizations into one session scorecard.
        assert!(
            single_blip_holds_baseline(),
            "governor must hold baseline bitrate through a single WAN blip"
        );
        assert_eq!(
            simulate_two_peer_shed_counting(false),
            0,
            "TrickleSkip must not inflate governor shed%"
        );
        let (_, shed) = governor_shed_counts(&[
            PushFate::Delivered,
            PushFate::TrickleSkip,
            PushFate::Delivered,
        ]);
        assert_eq!(shed, 0);

        // Mild WAN congestion (5% of GOP) under congestion-gated trickle —
        // not the old delta-starve path. Host push ~78 like Ricardo night.
        let paint = simulate_paint_fps(
            PaintSimConfig {
                skip_all_deltas_in_trickle: false,
                trickle_frames: 30,
                congestion_only_skip: true,
                congested_fraction: 0.05,
            },
            A::PUSH_FPS,
        );
        assert!(
            paint >= A::PAINT_FPS,
            "optimized paint={paint} must reach Ricardo's {ricardo}",
            ricardo = A::PAINT_FPS
        );

        let m = SessionMetrics {
            push_fps: A::PUSH_FPS,
            shed_pct: 0,
            encoder_kbps: A::ENCODER_KBPS,
            paint_fps: paint,
            input_s_p50_ms: 35.0, // edge kbm + 500Hz pad — under wow bar
        };
        assert!(
            beats_ricardo(m),
            "composed Joel path must beat Ricardo hard gate: {m:?}"
        );
    }

    #[test]
    fn frozen_self_scorecard_fails_beat_self_margin() {
        let m = SessionMetrics {
            push_fps: self_baseline::PUSH_FPS,
            shed_pct: self_baseline::SHED_PCT,
            encoder_kbps: self_baseline::ENCODER_KBPS,
            paint_fps: self_baseline::PAINT_FPS,
            input_s_p50_ms: self_baseline::SURPLUS_P50_MS,
        };
        assert!(beats_ricardo(m), "self baseline still clears Ricardo");
        assert!(
            !beats_self(m),
            "barely-Ricardo self scorecard must fail beat-self bars"
        );
    }

    #[test]
    fn target_session_beats_self() {
        let m = SessionMetrics {
            push_fps: 95.0,
            shed_pct: 0,
            encoder_kbps: A::ENCODER_KBPS,
            paint_fps: 105.0,
            input_s_p50_ms: 4.0,
        };
        assert!(beats_self(m));
    }

    #[test]
    fn old_delta_starve_path_cannot_beat_ricardo() {
        let paint = simulate_paint_fps(
            PaintSimConfig {
                skip_all_deltas_in_trickle: true,
                trickle_frames: 180,
                congestion_only_skip: false,
                congested_fraction: 0.0,
            },
            A::PUSH_FPS,
        );
        let m = SessionMetrics {
            push_fps: 0.8,
            shed_pct: 50,
            encoder_kbps: 1_250,
            paint_fps: paint,
            input_s_p50_ms: 35.0,
        };
        assert!(!beats_ricardo(m));
    }
}
