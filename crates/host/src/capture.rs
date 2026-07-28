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
