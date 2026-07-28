//! Screen / window capture via scrap (DXGI / X11 / Quartz) — same stack as Rohomieo.

use anyhow::{Context, Result};
use scrap::{Capturer, Display};

pub struct FrameCapture {
    capturer: Capturer,
    pub width: usize,
    pub height: usize,
}

impl FrameCapture {
    pub fn primary() -> Result<Self> {
        let display = Display::primary().context("no primary display")?;
        let width = display.width();
        let height = display.height();
        let capturer = Capturer::new(display).context("create capturer")?;
        Ok(Self {
            capturer,
            width,
            height,
        })
    }

    pub fn capture_bgra(&mut self) -> Result<Option<Vec<u8>>> {
        match self.capturer.frame() {
            Ok(frame) => Ok(Some(frame.to_vec())),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// Sample average luma from BGRA (0–255). Used once to detect empty/black capture.
pub fn sample_avg_luma_bgra(bgra: &[u8], max_pixels: usize) -> u64 {
    let mut sum = 0u64;
    let mut n = 0u64;
    for p in bgra.chunks_exact(4).take(max_pixels) {
        sum += (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3;
        n += 1;
    }
    if n == 0 {
        0
    } else {
        sum / n
    }
}
