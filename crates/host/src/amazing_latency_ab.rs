//! Amazing-interactive-latency adversarial A/B lock (math-impl T4).
//!
//! Locks the wow bars, WebCodecs path shape, and SHM-gate discipline so live
//! friend sessions measure against fixed invariants — not against drifting
//! comments. Bitrate/1080 climb stays **blocked** until live MATH-2 / AMAZE-1
//! (`S_p50 ≤ 45ms`) passes.
//!
//! Live checklist (paste on PR / friend night):
//! ```text
//! MATH-1 ricardo_wow Φ*=93 at R=48
//! MATH-2 live S_p50 ≤ 45ms (wow) — stretch 30 after handoff proof
//! MATH-3 present=webcodecs <3s Chrome
//! MATH-4 shm_gate decision documented (trip or skip)
//! MATH-5 no death-spiral / ricardo_playable_ab 7/7
//! MATH-6 drawer Latency tab shows Φ and S (est.), not push as hero
//! AMAZE-1 photon p50 ≤ RTT+45ms (friend drawer)
//! AMAZE-2 present=webcodecs on Chrome <3s
//! AMAZE-3 no 1Hz keyframe-budget spam / IDR storm
//! AMAZE-4 ricardo_playable_ab 7/7 + host units green
//! AMAZE-5 handoff wait p95 recorded (SHM only if gate)
//! ```

use crate::input_photon_budget::{
    handoff_wait_periods, photon_wow_ms, recommend_shm, shm_decision_label, shm_gate_trips,
    surplus_ms, wow_surplus_ok, RICARDO_RTT_MS, SHM_WAIT_P95_GATE_MS, STRETCH_SURPLUS_MS,
    WOW_SURPLUS_MS,
};
use crate::webrtc_peer::{path_flags, PATH_WEBCODECS};

/// Live soft scrape: photon p50 vs RTT must clear the wow bar.
pub fn live_photon_wow_ok(photon_p50_ms: f64, rtt_ms: f64) -> bool {
    wow_surplus_ok(surplus_ms(photon_p50_ms, rtt_ms))
}

/// Conjecture (optional live log — do not fail CI): \(S\) closer across LAN/WAN
/// than absolute \(\Phi\). Returns absolute |S_a − S_b| vs |\Phi_a − \Phi_b|.
pub fn surplus_closer_than_phi(
    phi_a: f64,
    rtt_a: f64,
    phi_b: f64,
    rtt_b: f64,
) -> (f64, f64) {
    let s_gap = (surplus_ms(phi_a, rtt_a) - surplus_ms(phi_b, rtt_b)).abs();
    let phi_gap = (phi_a - phi_b).abs();
    (s_gap, phi_gap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_photon_budget::photon_stretch_ms;
    use crate::ricardo_playable_ab::ricardo_playable_a as A;

    #[test]
    fn photon_wow_bar_is_rtt_plus_45() {
        assert!((photon_wow_ms(48.0) - 93.0).abs() < 1e-9);
        assert!((photon_wow_ms(RICARDO_RTT_MS) - (RICARDO_RTT_MS + WOW_SURPLUS_MS)).abs() < 1e-9);
    }

    #[test]
    fn stretch_bar_is_rtt_plus_30() {
        assert!(
            (photon_stretch_ms(RICARDO_RTT_MS) - (RICARDO_RTT_MS + STRETCH_SURPLUS_MS)).abs()
                < 1e-9
        );
    }

    #[test]
    fn webcodecs_path_still_clvd_only() {
        // Sacred: healthy WebCodecs = CLVD only (no dual full RTP send).
        assert_eq!(path_flags(PATH_WEBCODECS), (false, true));
    }

    #[test]
    fn live_photon_wow_uses_surplus_not_absolute_phi() {
        // Same S=42 under two RTTs → both pass; absolute Φ differs.
        assert!(live_photon_wow_ok(90.0, 48.0));
        assert!(live_photon_wow_ok(70.0, 28.0));
        assert!(!live_photon_wow_ok(100.0, 48.0)); // S=52 > 45
    }

    #[test]
    fn shm_gate_not_premature() {
        assert!(!shm_gate_trips(0.5));
        assert!(!recommend_shm(0.5));
        assert!(shm_decision_label(0.5).starts_with("SHM_SKIP"));
        assert!(recommend_shm(1.5));
    }

    #[test]
    fn handoff_omega_at_gate_is_small_fraction_of_frame() {
        let omega = handoff_wait_periods(SHM_WAIT_P95_GATE_MS, 60);
        assert!(
            omega < 0.1,
            "1ms wait at 60fps must be << one frame period"
        );
    }

    #[test]
    fn surplus_translation_symmetry_conjecture_shape() {
        // Optional live check shape: shifting Φ and R together leaves S; LAN vs WAN
        // should see smaller S gap than Φ gap when RTT dominates.
        let (s_gap, phi_gap) = surplus_closer_than_phi(90.0, 48.0, 70.0, 28.0);
        assert!((s_gap - 0.0).abs() < 1e-9); // both S=42
        assert!((phi_gap - 20.0).abs() < 1e-9);
        assert!(s_gap < phi_gap);
    }

    #[test]
    fn ricardo_playable_rtt_matches_budget_constant() {
        assert!((A::RTT_MS - RICARDO_RTT_MS).abs() < 1e-9);
    }

    #[test]
    fn live_sim_target_clears_wow_bar_at_ricardo_rtt() {
        use crate::latency_live_sim::{beats_ricardo, SessionMetrics};
        let m = SessionMetrics {
            push_fps: A::PUSH_FPS,
            shed_pct: 0,
            encoder_kbps: A::ENCODER_KBPS,
            paint_fps: A::PAINT_FPS,
            input_s_p50_ms: 35.0,
        };
        assert!(beats_ricardo(m));
        assert!(live_photon_wow_ok(35.0 + RICARDO_RTT_MS, RICARDO_RTT_MS));
    }

    #[test]
    fn mediocre_session_does_not_claim_beat_ricardo() {
        use crate::latency_live_sim::{beats_ricardo, SessionMetrics};
        assert!(!beats_ricardo(SessionMetrics {
            push_fps: 50.0,
            shed_pct: 8,
            encoder_kbps: A::ENCODER_KBPS,
            paint_fps: 70.0,
            input_s_p50_ms: 44.0,
        }));
    }
}
