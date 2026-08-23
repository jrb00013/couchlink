//! Input→photon budget math — surplus \(S = \Phi - R\) over RTT.
//!
//! Every constant cites `docs/superpowers/specs/2026-08-23-amazing-interactive-latency-math.md`
//! or a live observation. Do not invent a number here. Push fps is not the objective.
//!
//! Notation:
//! - \(\Phi\) = input→photon (ms), client paint − pad send of watermarked seq
//! - \(R\) = RTT (ms)
//! - \(S = \Phi - R\) = surplus over RTT (the true optimization target)
//! - \(\eta = S / R\) = surplus in RTT units
//! - Phase waits use mean unsync \(T/2\) (same as `wan3_math::mean_unsync_wait_ms`)

use crate::wan3_math::{mean_unsync_wait_ms, period_ms};

/// Ricardo playable-night RTT (ms). Source: session ~2026-08-23 02:08 / ricardo_playable_a.
pub const RICARDO_RTT_MS: f64 = 48.0;

/// First wow bar: \(S^\star = \Phi - R \le\) this (ms). Design + math doc.
pub const WOW_SURPLUS_MS: f64 = 45.0;

/// Stretch after handoff wait proven small / SHM.
pub const STRETCH_SURPLUS_MS: f64 = 30.0;

/// Absolute wait p95 (ms) that trips the SHM decision gate.
/// Math doc / design: material if wait p95 > 1.0 ms.
pub const SHM_WAIT_P95_GATE_MS: f64 = 1.0;

/// \(S = \Phi - R\).
pub fn surplus_ms(phi_ms: f64, rtt_ms: f64) -> f64 {
    phi_ms - rtt_ms
}

/// \(\eta = S / R\) (dimensionless). Returns 0 if RTT ≤ 0.
pub fn surplus_rtt_units(phi_ms: f64, rtt_ms: f64) -> f64 {
    if rtt_ms <= 0.0 {
        return 0.0;
    }
    surplus_ms(phi_ms, rtt_ms) / rtt_ms
}

/// \(\Phi^\star = R + S^\star\) at the first wow bar.
pub fn photon_wow_ms(rtt_ms: f64) -> f64 {
    rtt_ms + WOW_SURPLUS_MS
}

/// \(\Phi^\star\) at the stretch bar.
pub fn photon_stretch_ms(rtt_ms: f64) -> f64 {
    rtt_ms + STRETCH_SURPLUS_MS
}

/// Mean phase wait \(T/2\) for one periodic handoff (ms).
pub fn mean_phase_wait_ms(hz: u32) -> f64 {
    mean_unsync_wait_ms(hz)
}

/// Mean phase stack: pad + video + display (ms).
pub fn mean_phase_stack_ms(pad_hz: u32, video_fps: u32, display_fps: u32) -> f64 {
    mean_phase_wait_ms(pad_hz) + mean_phase_wait_ms(video_fps) + mean_phase_wait_ms(display_fps)
}

/// Residual inside the wow surplus after mean phase waits (ms).
pub fn wow_residual_after_phases_ms(pad_hz: u32, video_fps: u32, display_fps: u32) -> f64 {
    WOW_SURPLUS_MS - mean_phase_stack_ms(pad_hz, video_fps, display_fps)
}

/// \(\omega = w / T_v\) — handoff wait in video periods.
pub fn handoff_wait_periods(wait_ms: f64, video_fps: u32) -> f64 {
    let t = period_ms(video_fps);
    if t <= 0.0 {
        return 0.0;
    }
    wait_ms / t
}

/// SHM decision: trip if wait p95 exceeds the absolute gate.
pub fn shm_gate_trips(wait_p95_ms: f64) -> bool {
    wait_p95_ms > SHM_WAIT_P95_GATE_MS
}

/// Live wow check: \(S_{p50} \le S^\star\).
pub fn wow_surplus_ok(surplus_p50_ms: f64) -> bool {
    surplus_p50_ms <= WOW_SURPLUS_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn almost(a: f64, b: f64, eps: f64) {
        assert!((a - b).abs() < eps, "{a} ≉ {b} (eps={eps})");
    }

    #[test]
    fn ricardo_wow_photon_is_rtt_plus_45() {
        almost(photon_wow_ms(RICARDO_RTT_MS), 93.0, 1e-9);
        almost(surplus_ms(93.0, 48.0), 45.0, 1e-9);
        almost(surplus_rtt_units(93.0, 48.0), 45.0 / 48.0, 1e-9);
    }

    #[test]
    fn stretch_photon_is_rtt_plus_30() {
        almost(photon_stretch_ms(RICARDO_RTT_MS), 78.0, 1e-9);
    }

    #[test]
    fn mean_phase_stack_at_60_and_250_is_about_18_7() {
        // T_p/2=2, T_v/2≈8.333, T_d/2≈8.333 → ≈18.666...
        let s = mean_phase_stack_ms(250, 60, 60);
        almost(s, 1000.0 / 250.0 / 2.0 + 1000.0 / 60.0 / 2.0 + 1000.0 / 60.0 / 2.0, 1e-9);
        almost(s, 18.666_666_666_666_668, 0.01);
    }

    #[test]
    fn residual_after_phases_inside_wow_is_about_26_3() {
        let residual = wow_residual_after_phases_ms(250, 60, 60);
        almost(residual, 26.333_333, 0.01);
    }

    #[test]
    fn shm_gate_trips_above_one_ms_wait_p95() {
        assert!(!shm_gate_trips(0.4));
        assert!(!shm_gate_trips(1.0));
        assert!(shm_gate_trips(1.01));
    }

    #[test]
    fn surplus_is_translation_invariant_in_phi_and_r() {
        // Symmetry: shifting both Φ and R by Δ leaves S unchanged.
        let s1 = surplus_ms(90.0, 40.0);
        let s2 = surplus_ms(100.0, 50.0);
        almost(s1, s2, 1e-9);
    }

    #[test]
    fn handoff_omega_one_ms_at_60_is_small_fraction_of_period() {
        let omega = handoff_wait_periods(1.0, 60);
        almost(omega, 1.0 / period_ms(60), 1e-9);
        assert!(omega < 0.1);
    }

    #[test]
    fn wow_surplus_ok_at_bar() {
        assert!(wow_surplus_ok(45.0));
        assert!(wow_surplus_ok(44.9));
        assert!(!wow_surplus_ok(45.1));
    }
}
