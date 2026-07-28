//! TCP accept for `couchlink-win-capture` (Windows desktop → WSL host).
//! WSL listens; Windows connects out (avoids Windows inbound firewall).

use anyhow::{Context, Result};
use couchlink_capture_bridge::read_frame_sync;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

pub struct WindowsBridge {
    stream: TcpStream,
    pub width: usize,
    pub height: usize,
    buf: Vec<u8>,
    pending: Option<Vec<u8>>,
}

impl WindowsBridge {
    /// Listen for the Windows capture client (default `0.0.0.0:9876`).
    pub fn listen(bind: &str) -> Result<Self> {
        let listener = TcpListener::bind(bind)
            .with_context(|| format!("bind Windows capture listener on {bind}"))?;
        tracing::info!(
            "waiting for couchlink-win-capture to connect (Windows → {bind})…"
        );
        listener
            .set_nonblocking(false)
            .context("set blocking accept")?;
        // Generous accept wait — ensure-win-capture may still be building the exe.
        let _ = listener.set_nonblocking(false);
        let (stream, peer) = listener
            .accept()
            .context("accept Windows capture client (is couchlink-win-capture running?)")?;
        tracing::info!("Windows capture client connected from {peer}");
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_nodelay(true).ok();
        let mut bridge = Self {
            stream,
            width: 0,
            height: 0,
            buf: Vec::new(),
            pending: None,
        };
        bridge.read_one()?;
        Ok(bridge)
    }

    fn read_one(&mut self) -> Result<()> {
        let info = read_frame_sync(&mut self.stream, &mut self.buf)?;
        self.width = info.width as usize;
        self.height = info.height as usize;
        self.pending = Some(self.buf.clone());
        Ok(())
    }

    pub fn capture_bgra(&mut self) -> Result<Option<Vec<u8>>> {
        if let Some(p) = self.pending.take() {
            return Ok(Some(p));
        }
        match read_frame_sync(&mut self.stream, &mut self.buf) {
            Ok(_) => Ok(Some(self.buf.clone())),
            Err(e) => {
                if let Some(io) = e.downcast_ref::<std::io::Error>() {
                    if io.kind() == std::io::ErrorKind::TimedOut {
                        return Ok(None);
                    }
                }
                Err(e)
            }
        }
    }
}
