//! Local display capture via scrap (X11 / WSLg / DXGI on native Windows host).

use anyhow::{Context, Result};
use scrap::{Capturer, Display};

pub struct ScrapCapture {
    capturer: Capturer,
    pub width: usize,
    pub height: usize,
}

impl ScrapCapture {
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
            Ok(frame) => Ok(Some(pack_tight_bgra(&frame, self.width, self.height))),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

pub fn pack_tight_bgra(frame: &[u8], width: usize, height: usize) -> Vec<u8> {
    let row_bytes = width * 4;
    let stride = if height > 0 {
        frame.len() / height
    } else {
        row_bytes
    };
    if stride == row_bytes {
        return frame.to_vec();
    }
    let mut tight = vec![0u8; row_bytes * height];
    for y in 0..height {
        let src = y * stride;
        let dst = y * row_bytes;
        if src + row_bytes <= frame.len() {
            tight[dst..dst + row_bytes].copy_from_slice(&frame[src..src + row_bytes]);
        }
    }
    tight
}

pub fn sample_avg_luma_bgra(bgra: &[u8], _width: usize, max_pixels: usize) -> u64 {
    let mut sum = 0u64;
    let mut n = 0u64;
    for p in bgra.chunks_exact(4).take(max_pixels.min(bgra.len() / 4)) {
        sum += (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3;
        n += 1;
    }
    if n == 0 {
        0
    } else {
        sum / n
    }
}
