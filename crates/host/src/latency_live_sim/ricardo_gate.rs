//! Ricardo beat scorecard — all three axes in one gate.

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

pub fn beats_ricardo(m: SessionMetrics) -> bool {
    m.push_fps >= A::MIN_PUSH_FPS
        && m.shed_pct <= A::MAX_SHED_PCT
        && m.encoder_kbps >= A::ENCODER_KBPS
        && m.paint_fps >= 70.0
        && m.input_s_p50_ms <= 45.0
}

pub fn beats_ricardo_push_and_paint(m: SessionMetrics) -> bool {
    m.push_fps >= A::PUSH_FPS * 0.9 && m.paint_fps >= A::PAINT_FPS * 0.95
}

#[cfg(test)]
mod tests {
    use super::*;

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
            push_fps: 72.0,
            shed_pct: 2,
            encoder_kbps: 5_000,
            paint_fps: 74.0,
            input_s_p50_ms: 35.0,
        };
        assert!(beats_ricardo(m));
    }
}
