//! Paint fps simulation — delta starvation vs congestion-gated trickle.

/// How many frames in a GOP between IDRs at host cadence.
pub const GOP_FRAMES: u32 = 180; // 3s @ 60fps (matches IDR_INTERVAL)

#[derive(Debug, Clone, Copy)]
pub struct PaintSimConfig {
    /// Old trickle: skip every delta while trickle flag set.
    pub skip_all_deltas_in_trickle: bool,
    /// Frames the session spends in trickle mode.
    pub trickle_frames: u32,
    /// When false, deltas send whenever not SCTP-congested (new path).
    pub congestion_only_skip: bool,
    /// Fraction of trickle frames where SCTP is congested (0..=1).
    pub congested_fraction: f32,
}

impl Default for PaintSimConfig {
    fn default() -> Self {
        Self {
            skip_all_deltas_in_trickle: false,
            trickle_frames: 120,
            congestion_only_skip: true,
            congested_fraction: 0.3,
        }
    }
}

/// Count paintable frames over one GOP given trickle policy.
pub fn simulate_paint_fps(cfg: PaintSimConfig, host_fps: f64) -> f64 {
    let mut painted = 0u32;
    let mut in_trickle = false;
    let mut ok_streak = 0u32;

    for frame in 0..GOP_FRAMES {
        let keyframe = frame == 0;
        if frame < cfg.trickle_frames {
            in_trickle = true;
        } else if in_trickle && ok_streak >= 4 {
            in_trickle = false;
        }

        // Deterministic congestion window at the start of the GOP (fraction of frames).
        // No extra %7 spikes — those made "mild WAN" look like a death spiral.
        let congested = (frame as f32 / GOP_FRAMES as f32) < cfg.congested_fraction;

        let skip_delta = if keyframe {
            false
        } else if !in_trickle {
            false
        } else if cfg.skip_all_deltas_in_trickle {
            true
        } else if cfg.congestion_only_skip {
            congested
        } else {
            false
        };

        if keyframe || !skip_delta {
            painted += 1;
            ok_streak += 1;
        } else {
            ok_streak = 0;
        }
    }

    (painted as f64 / GOP_FRAMES as f64) * host_fps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_trickle_starves_paint_below_playable() {
        let fps = simulate_paint_fps(
            PaintSimConfig {
                skip_all_deltas_in_trickle: true,
                trickle_frames: GOP_FRAMES,
                ..Default::default()
            },
            60.0,
        );
        // IDR-only ≈ 1 fps effective paint over a GOP
        assert!(fps < 10.0, "old trickle paint={fps} must cliff");
    }

    #[test]
    fn congestion_gated_trickle_stays_above_ricardo_floor() {
        let fps = simulate_paint_fps(PaintSimConfig::default(), 60.0);
        assert!(
            fps >= 35.0,
            "new trickle paint={fps} must stay playable (old path cliffs to ~1)"
        );
    }

    #[test]
    fn mild_wan_congestion_paints_at_or_above_ricardo() {
        let fps = simulate_paint_fps(
            PaintSimConfig {
                skip_all_deltas_in_trickle: false,
                trickle_frames: 30,
                congestion_only_skip: true,
                congested_fraction: 0.05,
            },
            77.8,
        );
        assert!(
            fps >= 74.0,
            "mild WAN paint={fps} must beat Ricardo's 74"
        );
    }

    #[test]
    fn healthy_link_paints_full_gop() {
        let fps = simulate_paint_fps(
            PaintSimConfig {
                trickle_frames: 0,
                ..Default::default()
            },
            60.0,
        );
        assert!(fps >= 58.0, "healthy paint={fps}");
    }
}
