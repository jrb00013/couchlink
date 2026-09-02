//! Link governor session replay — yo-yo and hysteresis gates.

use crate::link_gov::LinkGov;
use crate::ricardo_playable_ab::ricardo_playable_a as A;
use couchlink_capture_bridge::EncodeTarget;

// Must track ricardo_playable_ab::ricardo_playable_a::ENCODER_KBPS — the
// production preset moved to 10 Mbps (343ce8e, "production motion — 10Mbps@60,
// no B-frames") but this simulated baseline was left at the old 5 Mbps value,
// so the governor here never reached the bitrate every test compared it
// against (A::ENCODER_KBPS). Session replay must start from what the encoder
// actually ships, not a stale constant.
const PRODUCTION_BASELINE: EncodeTarget = EncodeTarget {
    width: 1280,
    height: 720,
    fps: 60,
    bitrate_kbps: A::ENCODER_KBPS,
};

#[derive(Debug, Clone)]
pub struct GovernorSessionResult {
    pub kbps_timeline: Vec<u32>,
    pub final_kbps: u32,
    pub stepped_down_on_blip: bool,
}

/// Replay shed/sent windows through LinkGov; returns commanded kbps each step.
pub fn simulate_governor_session(windows: &[(u32, u32)]) -> GovernorSessionResult {
    let mut gov = LinkGov::new(PRODUCTION_BASELINE);
    let baseline_kbps = PRODUCTION_BASELINE.bitrate_kbps;
    let mut kbps_timeline = Vec::with_capacity(windows.len());
    let mut stepped_down_on_blip = false;

    for (i, &(shed, sent)) in windows.iter().enumerate() {
        let t = gov.on_window(shed, sent);
        kbps_timeline.push(t.bitrate_kbps);
        if i == 0 && shed * 100 / sent.max(1) > 8 && t.bitrate_kbps < baseline_kbps {
            stepped_down_on_blip = false; // hysteresis: first blip alone must not step
        }
        if i == 1 && shed * 100 / sent.max(1) > 8 && t.bitrate_kbps < baseline_kbps {
            stepped_down_on_blip = true;
        }
    }

    GovernorSessionResult {
        final_kbps: *kbps_timeline.last().unwrap_or(&baseline_kbps),
        kbps_timeline,
        stepped_down_on_blip,
    }
}

/// Single 10% shed blip then clean — must hold baseline bitrate (Ricardo encoder target).
pub fn single_blip_holds_baseline() -> bool {
    let r = simulate_governor_session(&[(10, 100), (0, 60), (0, 60), (0, 60)]);
    r.kbps_timeline[0] == A::ENCODER_KBPS && r.kbps_timeline[1] == A::ENCODER_KBPS
}

/// Sustained 40% shed must reach floor without dropping fps.
pub fn sustained_shed_reaches_floor_not_625() -> bool {
    let windows: Vec<(u32, u32)> = (0..20).map(|_| (40, 100)).collect();
    let r = simulate_governor_session(&windows);
    r.final_kbps >= 1_250 && r.final_kbps != 625
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_wan_blip_does_not_yo_yo_encoder() {
        assert!(single_blip_holds_baseline());
    }

    #[test]
    fn sustained_congestion_floors_at_1250_not_625() {
        assert!(sustained_shed_reaches_floor_not_625());
    }

    #[test]
    fn recovering_session_climbs_back_to_baseline() {
        let mut windows: Vec<(u32, u32)> = (0..10).map(|_| (40, 100)).collect();
        windows.extend((0..40).map(|_| (0, 60)));
        let r = simulate_governor_session(&windows);
        assert_eq!(r.final_kbps, A::ENCODER_KBPS);
    }
}
