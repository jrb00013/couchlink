//! Tile-diff motion detector — Rohomieo methodology.
//! Skips encode / drops to idle FPS when <2% of sampled tiles change.

pub struct MotionDetector {
    prev: Vec<u8>,
    width: u32,
    height: u32,
    tile: u32,
}

impl MotionDetector {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            prev: Vec::new(),
            width,
            height,
            tile: 32,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.prev.clear();
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
        let need = (tiles_x * tiles_y) as usize;
        if self.prev.len() != need {
            self.prev = vec![0u8; need];
        }
        let mut changed = 0u32;
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let x = (tx * tw + tw / 2) as usize;
                let y = (ty * tw + tw / 2) as usize;
                let i = y * stride + x * 4;
                let sample = bgra[i] ^ bgra[i + 1] ^ bgra[i + 2];
                let idx = (ty * tiles_x + tx) as usize;
                if self.prev[idx] != sample {
                    changed += 1;
                    self.prev[idx] = sample;
                }
            }
        }
        changed as f32 / need as f32
    }

    pub fn is_idle(&mut self, bgra: &[u8]) -> bool {
        self.changed_fraction(bgra) < 0.02
    }
}
