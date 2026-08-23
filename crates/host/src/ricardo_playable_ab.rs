//! A/B regression gate: Ricardo's "damn this is playable" session (2026-08-23 ~02:08)
//! vs the invariants that must not regress.
//!
//! **A (baseline)** — live drawer from Ricardo B. on canvas / RTP, feeling good:
//! push 0.1ms · 77.8fps · 0% shed · 5.00 Mbps@60 · capture ~1.7ms · paint 74 ·
//! decode 82 · RTT 48ms · freeze 0.
//!
//! **B (must hold)** — code + math that keep that shape reachable. If these fail,
//! we have walked back into the IDR / dual-send / fps-drop death spiral.

use couchlink_capture_bridge::EncodeTarget;
use couchlink_proto::StreamPreset;

use crate::wan3_math::{host_uplink_kbps, rungs_from, N_FRIENDS, PATHS_WEBCODECS};

/// Ricardo playable drawer — frozen observation, not a wish.
pub mod ricardo_playable_a {
    pub const PUSH_MS: f64 = 0.1;
    pub const PUSH_FPS: f64 = 77.8;
    pub const SHED_PCT: u32 = 0;
    pub const ENCODER_W: u32 = 1280;
    pub const ENCODER_H: u32 = 720;
    pub const ENCODER_FPS: u32 = 60;
    pub const ENCODER_KBPS: u32 = 5_000;
    pub const CAPTURE_MS: f64 = 1.7;
    pub const PAINT_FPS: f64 = 74.0;
    pub const DECODE_FPS: f64 = 82.0;
    pub const RTT_MS: f64 = 48.0;
    pub const FREEZE_FRAMES: u32 = 0;
    /// Soft floors for live A/B scrapes (allow jitter around the playable night).
    pub const MIN_PUSH_FPS: f64 = 50.0;
    pub const MAX_PUSH_MS: f64 = 10.0;
    pub const MAX_SHED_PCT: u32 = 8;
    pub const MAX_CAPTURE_MS: f64 = 5.0;
}

#[cfg(test)]
mod tests {
    use super::ricardo_playable_a as A;
    use super::*;
    use crate::webrtc_peer::{
        path_flags, should_enter_trickle, should_exit_trickle, should_send_rtp, PATH_UNKNOWN,
        PATH_WEBCODECS,
    };

    /// Live preset friends joined with that night.
    fn playable_preset() -> EncodeTarget {
        let p = StreamPreset::P720_60;
        EncodeTarget {
            width: p.width,
            height: p.height,
            fps: p.fps,
            bitrate_kbps: p.bitrate_kbps,
        }
    }

    #[test]
    fn a_baseline_encoder_matches_720p60_at_5mbps() {
        let p = playable_preset();
        assert_eq!(p.width, A::ENCODER_W);
        assert_eq!(p.height, A::ENCODER_H);
        assert_eq!(p.fps, A::ENCODER_FPS);
        assert_eq!(p.bitrate_kbps, A::ENCODER_KBPS);
    }

    #[test]
    fn b_fps_hold_never_drops_below_playable_60() {
        for r in rungs_from(&playable_preset()) {
            assert_eq!(
                r.fps, A::ENCODER_FPS,
                "fps-hold broken vs Ricardo playable: {r:?}"
            );
        }
    }

    #[test]
    fn b_bitrate_floor_never_returns_unplayable_625() {
        let floor = *rungs_from(&playable_preset()).last().unwrap();
        assert!(
            floor.bitrate_kbps >= 1_250,
            "625 kbps was the death spiral; floor={floor:?}"
        );
        assert_ne!(floor.bitrate_kbps, 625);
    }

    #[test]
    fn b_healthy_webcodecs_is_one_path_like_playable_uplink() {
        // Playable night: push 0.1ms — dual full send cannot stay there on 3 WAN friends.
        assert_eq!(PATHS_WEBCODECS, 1);
        assert_eq!(
            host_uplink_kbps(A::ENCODER_KBPS, N_FRIENDS, PATHS_WEBCODECS),
            15_000
        );
        assert_eq!(path_flags(PATH_WEBCODECS), (false, true));
        assert!(!should_send_rtp(false, PATH_WEBCODECS, false));
        assert!(!should_send_rtp(true, PATH_WEBCODECS, false));
    }

    #[test]
    fn b_warmup_still_dual_so_join_is_not_black() {
        assert_eq!(path_flags(PATH_UNKNOWN), (true, true));
        assert!(should_send_rtp(false, PATH_UNKNOWN, false));
    }

    #[test]
    fn b_trickle_isolates_slow_peer_without_killing_healthy() {
        assert!(should_enter_trickle(8));
        assert!(!should_enter_trickle(7));
        assert!(should_exit_trickle(4));
        assert!(!should_exit_trickle(3));
    }

    #[test]
    fn b_live_sim_target_beats_ricardo_on_all_axes() {
        use crate::latency_live_sim::{beats_ricardo, SessionMetrics};
        // Hard bars: ≥Ricardo paint (74), ≤3% shed, hold 5 Mbps, S≤45.
        let m = SessionMetrics {
            push_fps: A::PUSH_FPS,
            shed_pct: 0,
            encoder_kbps: A::ENCODER_KBPS,
            paint_fps: A::PAINT_FPS,
            input_s_p50_ms: 35.0,
        };
        assert!(beats_ricardo(m));
        // Soft-floor mediocrity must not pass the hard gate.
        let soft = SessionMetrics {
            push_fps: 50.0,
            shed_pct: 8,
            encoder_kbps: A::ENCODER_KBPS,
            paint_fps: 70.0,
            input_s_p50_ms: 40.0,
        };
        assert!(!beats_ricardo(soft));
    }

    #[test]
    fn a_playable_soft_gates_are_internally_consistent() {
        // Capture-bound session: push ≪ capture, shed ~0, paint tracks decode.
        assert!(A::PUSH_MS < A::CAPTURE_MS);
        assert!(A::PUSH_FPS >= A::MIN_PUSH_FPS);
        assert!(A::PAINT_FPS >= 45.0);
        assert_eq!(A::SHED_PCT, 0);
        assert_eq!(A::FREEZE_FRAMES, 0);
        assert!(A::RTT_MS < 100.0);
    }
}
