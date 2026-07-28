//! TCP client for `couchlink-win-capture` (Windows desktop → WSL host).

use anyhow::{Context, Result};
use couchlink_capture_bridge::read_frame_sync;
use std::net::TcpStream;
use std::time::Duration;

pub struct WindowsBridge {
    stream: TcpStream,
    pub width: usize,
    pub height: usize,
    buf: Vec<u8>,
    pending: Option<Vec<u8>>,
}

impl WindowsBridge {
    pub fn connect(addr: &str) -> Result<Self> {
        let stream = TcpStream::connect(addr)
            .with_context(|| format!("connect Windows capture bridge at {addr} (is couchlink-win-capture running on Windows?)"))?;
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
