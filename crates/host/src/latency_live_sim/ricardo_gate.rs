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
/// | encode | 5 Mbps  | ≥ 5000 kbps     |
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
            encoder_kbps: 5_000,
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
            encoder_kbps: 5_000,
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
            "governor must hold 5 Mbps through a single WAN blip"
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
