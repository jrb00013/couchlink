//! TCP accept for `couchlink-win-capture` (Windows desktop → WSL host).
//! WSL listens; Windows connects out (avoids Windows inbound firewall).

use anyhow::{bail, Context, Result};
use couchlink_capture_bridge::{read_frame_body_sync, FrameInfo, FRAME_MAGIC};
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// How long to wait for a frame to start arriving. Deliberately tiny: the caller
/// sends on a fixed cadence, so this must return promptly with either a fresh frame
/// or the previous one. Blocking here would stall that metronome and reintroduce the
/// arrival jitter the cadence exists to remove.
const IDLE_POLL: Duration = Duration::from_millis(2);
/// Poll used while draining a backlog — only frames already buffered count.
const DRAIN_POLL: Duration = Duration::from_millis(1);
/// Once a frame has started, the rest is already in flight — allow plenty of time.
const FRAME_BODY_TIMEOUT: Duration = Duration::from_secs(10);

/// A socket read timeout surfaces as `WouldBlock` (EAGAIN) on Unix and `TimedOut`
/// on Windows. Both mean "no data yet", not "connection broken".
fn is_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

pub struct WindowsBridge {
    listener: TcpListener,
    /// `None` while the Windows client is away; the bridge keeps serving the last
    /// frame and re-accepts in the background instead of taking the host down.
    stream: Option<TcpStream>,
    pub width: usize,
    pub height: usize,
    buf: Vec<u8>,
    pending: Option<Vec<u8>>,
    /// Last frame received, re-served when the captured window is static or the
    /// client is reconnecting. Without this the encoder never runs, so a requested
    /// IDR never reaches a late-joining browser and the player sits on black.
    last: Option<Vec<u8>>,
}

impl WindowsBridge {
    /// Listen for the Windows capture client (default `0.0.0.0:9876`).
    pub fn listen(bind: &str) -> Result<Self> {
        let listener = TcpListener::bind(bind)
            .with_context(|| format!("bind Windows capture listener on {bind}"))?;
        tracing::info!("waiting for couchlink-win-capture to connect (Windows → {bind})…");
        // Generous accept wait — ensure-win-capture may still be building the exe.
        listener.set_nonblocking(false).context("blocking accept")?;
        let (stream, peer) = listener
            .accept()
            .context("accept Windows capture client (is couchlink-win-capture running?)")?;
        tracing::info!("Windows capture client connected from {peer}");
        configure(&stream)?;
        // From here on, reconnects must never block the host's frame loop.
        listener.set_nonblocking(true).context("nonblocking accept")?;
        let mut bridge = Self {
            listener,
            stream: Some(stream),
            width: 0,
            height: 0,
            buf: Vec::new(),
            pending: None,
            last: None,
        };
        bridge.read_one()?;
        Ok(bridge)
    }

    fn read_one(&mut self) -> Result<()> {
        loop {
            if self.read_frame(IDLE_POLL)?.is_some() {
                self.pending = Some(self.buf.clone());
                return Ok(());
            }
        }
    }

    /// Pick up a new client after a disconnect. Non-blocking: `false` means nobody
    /// is waiting yet, which is normal while win-capture restarts.
    fn try_reconnect(&mut self) -> bool {
        match self.listener.accept() {
            Ok((stream, peer)) => {
                if configure(&stream).is_err() {
                    return false;
                }
                tracing::info!("Windows capture client reconnected from {peer}");
                self.stream = Some(stream);
                true
            }
            Err(_) => false,
        }
    }

    /// `Ok(None)` means "no frame started within `poll`" — normal when the captured
    /// window is static and windows-capture has nothing new to send.
    fn read_frame(&mut self, poll: Duration) -> Result<Option<FrameInfo>> {
        let Some(stream) = self.stream.as_mut() else {
            return Ok(None);
        };
        stream.set_read_timeout(Some(poll))?;
        let mut magic = [0u8; 4];
        let mut got = 0;
        while got < 4 {
            match stream.read(&mut magic[got..]) {
                Ok(0) => bail!("Windows capture client disconnected"),
                Ok(n) => got += n,
                // Only a timeout *before any byte of the frame* is safe to shrug off.
                Err(ref e) if is_timeout(e) && got == 0 => return Ok(None),
                Err(e) => return Err(e).context("frame magic"),
            }
        }
        if &magic != FRAME_MAGIC {
            bail!("bad frame magic {magic:?} — capture stream desynchronized");
        }
        stream.set_read_timeout(Some(FRAME_BODY_TIMEOUT))?;
        let info = read_frame_body_sync(stream, &mut self.buf)?;
        // Dimensions can change mid-stream when the captured window is resized.
        // Callers scale against width()/height(), so these must track every frame
        // or they will index past the end of a smaller buffer.
        if self.width != info.width as usize || self.height != info.height as usize {
            if self.width != 0 {
                tracing::info!(
                    "capture size changed {}x{} → {}x{}",
                    self.width,
                    self.height,
                    info.width,
                    info.height
                );
            }
            self.width = info.width as usize;
            self.height = info.height as usize;
            // A stale frame at the old size would be misread at the new size.
            self.last = None;
        }
        Ok(Some(info))
    }

    /// Read the newest available frame, discarding any backlog. The socket buffers
    /// whole frames when the encoder falls behind; consuming them in order would add
    /// permanent, ever-growing latency, so only the most recent one is kept.
    fn latest_frame(&mut self) -> Result<bool> {
        if self.read_frame(IDLE_POLL)?.is_none() {
            return Ok(false);
        }
        let mut dropped = 0u32;
        while self.read_frame(DRAIN_POLL)?.is_some() {
            dropped += 1;
        }
        if dropped > 0 {
            tracing::debug!("dropped {dropped} stale capture frame(s) to stay live");
        }
        Ok(true)
    }

    pub fn capture_bgra(&mut self) -> Result<Option<Vec<u8>>> {
        if let Some(p) = self.pending.take() {
            self.last = Some(p.clone());
            return Ok(Some(p));
        }
        if self.stream.is_none() {
            self.try_reconnect();
            return Ok(self.last.clone());
        }
        match self.latest_frame() {
            Ok(true) => {
                // One copy, not two. At 720p each clone is 3.3MB and this runs on
                // every frame; the old code cloned once for `last` and again for the
                // caller.
                let frame = std::mem::take(&mut self.buf);
                self.last = Some(frame.clone());
                Ok(Some(frame))
            }
            Ok(false) => Ok(self.last.clone()),
            // A dead client must not kill the session: keep showing the last frame
            // and wait for win-capture to come back.
            Err(e) => {
                tracing::warn!("Windows capture client lost ({e:#}) — waiting for reconnect");
                self.stream = None;
                self.try_reconnect();
                Ok(self.last.clone())
            }
        }
    }
}

fn configure(stream: &TcpStream) -> Result<()> {
    stream.set_read_timeout(Some(FRAME_BODY_TIMEOUT))?;
    stream.set_nodelay(true).ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    /// Regression: a socket read timeout is `WouldBlock` (EAGAIN, os error 11) on
    /// Linux and `TimedOut` on Windows. Treating only `TimedOut` as recoverable made
    /// the host exit with "frame magic: Resource temporarily unavailable" the first
    /// time the captured window went quiet for longer than the read timeout.
    #[test]
    fn both_timeout_kinds_are_recoverable() {
        assert!(is_timeout(&std::io::Error::from(ErrorKind::WouldBlock)));
        assert!(is_timeout(&std::io::Error::from(ErrorKind::TimedOut)));
        assert!(is_timeout(&std::io::Error::from_raw_os_error(11)));
    }

    #[test]
    fn real_failures_are_not_mistaken_for_timeouts() {
        assert!(!is_timeout(&std::io::Error::from(ErrorKind::ConnectionReset)));
        assert!(!is_timeout(&std::io::Error::from(ErrorKind::UnexpectedEof)));
        assert!(!is_timeout(&std::io::Error::from(ErrorKind::BrokenPipe)));
    }
}
