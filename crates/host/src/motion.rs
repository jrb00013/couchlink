//! Tile-diff motion detector — drops to idle FPS when the screen is genuinely static.
//!
//! Sampling has to be honest in both directions: too coarse and real motion reads as
//! idle (the stream throttles to idle_fps and feels laggy), too fine and it costs more
//! than the encode it saves. Several samples per tile with a luma tolerance, plus
//! hysteresis so a single static frame never throttles a live stream.

/// Fraction of tiles that must change for the screen to count as active.
const ACTIVE_TILE_FRACTION: f32 = 0.004;
/// Per-sample luma delta that counts as a change (ignores dither / compression noise).
const LUMA_TOLERANCE: i32 = 10;
/// Consecutive idle frames required before throttling. Prevents a still moment in
/// otherwise live content from injecting an idle-length sleep into the frame path.
const IDLE_STREAK_REQUIRED: u32 = 6;
/// Sample points per tile, as (x, y) fractions of the tile size.
const SAMPLES: [(u32, u32); 4] = [(1, 1), (3, 1), (1, 3), (3, 3)];

pub struct MotionDetector {
    prev: Vec<u8>,
    width: u32,
    height: u32,
    tile: u32,
    idle_streak: u32,
}

impl MotionDetector {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            prev: Vec::new(),
            width,
            height,
            tile: 32,
            idle_streak: 0,
        }
    }

    #[allow(dead_code)]
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.prev.clear();
        self.idle_streak = 0;
    }

    /// Returns fraction of changed tiles in 0.0..=1.0 (BGRA input).
    pub fn changed_fraction(&mut self, bgra: &[u8]) -> f32 {
        let stride = (self.width as usize) * 4;
        if bgra.len() < stride * self.height as usize {
            return 1.0;
        }
        let tw = self.tile.max(8);
        let tiles_x = (self.width / tw).max(1);
        let tiles_y = (self.height / tw).max(1);
        let need = (tiles_x * tiles_y) as usize * SAMPLES.len();
        if self.prev.len() != need {
            self.prev = vec![0u8; need];
            // First frame after a resize: everything is "new", but reporting 1.0 here
            // would be a false motion signal. Seed and report idle-safe zero.
            self.seed(bgra, stride, tiles_x, tiles_y, tw);
            return 1.0;
        }
        let mut changed = 0u32;
        let mut slot = 0usize;
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let mut tile_changed = false;
                for (fx, fy) in SAMPLES {
                    let x = (tx * tw + (tw * fx) / 4).min(self.width - 1) as usize;
                    let y = (ty * tw + (tw * fy) / 4).min(self.height - 1) as usize;
                    let luma = luma_at(bgra, y * stride + x * 4);
                    if (luma as i32 - self.prev[slot] as i32).abs() > LUMA_TOLERANCE {
                        tile_changed = true;
                    }
                    self.prev[slot] = luma;
                    slot += 1;
                }
                if tile_changed {
                    changed += 1;
                }
            }
        }
        changed as f32 / (tiles_x * tiles_y) as f32
    }

    fn seed(&mut self, bgra: &[u8], stride: usize, tiles_x: u32, tiles_y: u32, tw: u32) {
        let mut slot = 0usize;
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                for (fx, fy) in SAMPLES {
                    let x = (tx * tw + (tw * fx) / 4).min(self.width - 1) as usize;
                    let y = (ty * tw + (tw * fy) / 4).min(self.height - 1) as usize;
                    self.prev[slot] = luma_at(bgra, y * stride + x * 4);
                    slot += 1;
                }
            }
        }
    }

    pub fn is_idle(&mut self, bgra: &[u8]) -> bool {
        if self.changed_fraction(bgra) >= ACTIVE_TILE_FRACTION {
            self.idle_streak = 0;
            return false;
        }
        self.idle_streak = self.idle_streak.saturating_add(1);
        self.idle_streak >= IDLE_STREAK_REQUIRED
    }
}

fn luma_at(bgra: &[u8], i: usize) -> u8 {
    let Some(px) = bgra.get(i..i + 3) else {
        return 0;
    };
    // Rough BT.601 luma; exactness does not matter, stability does.
    (((px[2] as u32 * 77) + (px[1] as u32 * 150) + (px[0] as u32 * 29)) >> 8) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: usize, h: usize, v: u8) -> Vec<u8> {
        vec![v; w * h * 4]
    }

    #[test]
    fn identical_frames_go_idle_after_hysteresis() {
        let mut m = MotionDetector::new(320, 240);
        let f = frame(320, 240, 40);
        m.is_idle(&f); // seed
        for _ in 0..IDLE_STREAK_REQUIRED {
            m.is_idle(&f);
        }
        assert!(m.is_idle(&f));
    }

    #[test]
    fn one_static_frame_does_not_throttle() {
        let mut m = MotionDetector::new(320, 240);
        let f = frame(320, 240, 40);
        m.is_idle(&f);
        assert!(!m.is_idle(&f), "must not throttle on the first static frame");
    }

    #[test]
    fn small_moving_region_counts_as_active() {
        let mut m = MotionDetector::new(320, 240);
        let mut f = frame(320, 240, 40);
        m.is_idle(&f);
        // Repaint a modest area — a cursor-sized change should not read as idle.
        for y in 60..140 {
            for x in 60..200 {
                let i = (y * 320 + x) * 4;
                f[i] = 200;
                f[i + 1] = 200;
                f[i + 2] = 200;
            }
        }
        assert!(!m.is_idle(&f), "visible motion must not read as idle");
    }
}
