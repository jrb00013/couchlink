//! TCP accept for `couchlink-win-capture` (Windows desktop → WSL host).
//! WSL listens; Windows connects out (avoids Windows inbound firewall).

use anyhow::{bail, Context, Result};
use couchlink_capture_bridge::audio::{read_audio_body, AudioFrame, AUDIO_MAGIC};
use couchlink_capture_bridge::{
    read_frame_body_sync, write_set_target, EncodeTarget, FrameFormat, FrameInfo, FRAME_MAGIC,
    EXPEDITE, REQUEST_IDR,
};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// See `hyperv_bridge::MAX_PENDING_AUDIO` — same cap, same reasoning.
const MAX_PENDING_AUDIO: usize = 100;

/// How long to wait for a frame to start arriving. Deliberately tiny: the caller
/// sends on a fixed cadence, so this must return promptly with either a fresh frame
/// or the previous one. Blocking here would stall that metronome and reintroduce the
/// arrival jitter the cadence exists to remove.
const IDLE_POLL: Duration = Duration::from_millis(2);
/// Poll used while draining a backlog — only frames already buffered count.
const DRAIN_POLL: Duration = Duration::from_millis(1);
/// How long win-capture may be gone before we relaunch it ourselves — see
/// `super::respawn_windows_capture`. Long enough that a normal reconnect (a
/// picker window closing and reopening, a brief TCP blip) never triggers it.
///
/// Was 5s. Issue #67: a hung-not-disconnected win-capture froze the stream
/// for ~10s — almost exactly `FRAME_STALE_AFTER` (4s) + this grace (5s) +
/// relaunch/reconnect time. Neither threshold needs to be that generous: a
/// picker reopen or brief TCP blip resolves in well under a second in
/// practice, so 2s still filters those out while cutting the visible freeze
/// roughly in half.
const RESPAWN_AFTER: Duration = Duration::from_secs(2);
/// Floor between relaunch attempts, so a win-capture that keeps failing to
/// start doesn't get hammered every capture-poll tick.
const RESPAWN_RETRY_INTERVAL: Duration = Duration::from_secs(20);
/// Hung-alive (socket reconnects, no frames): recover faster than the crash-loop
/// floor. 20s left friends on black "webcodecs video" after each hang (2026-10-12 MK).
const HUNG_RESPAWN_RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// Once a frame has started, the rest is already in flight — allow plenty of time.
const FRAME_BODY_TIMEOUT: Duration = Duration::from_secs(10);
/// A hung (not crashed/disconnected) win-capture never trips `Err` from
/// `read_frame` — the socket stays open, `latest_frame` just keeps returning
/// `Ok(false)` forever because nothing new ever arrives. That silently starves
/// `maybe_respawn`, which only fires off a real socket error. This is the
/// second trigger: if no frame has landed in this long while the socket is
/// still nominally connected, treat it exactly like a disconnect.
///
/// Was 4s — over 200 missed frames' worth of margin at 60fps. Issue #67:
/// this generosity directly extended the visible freeze when win-capture
/// hung. 1.5s is still ~90 missed frames of slack, far past any real jitter,
/// while detecting the hang much sooner.
const FRAME_STALE_AFTER: Duration = Duration::from_millis(1500);

/// A socket read timeout surfaces as `WouldBlock` (EAGAIN) on Unix and `TimedOut`
/// on Windows. Both mean "no data yet", not "connection broken".
fn is_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// What the Windows side delivered. H264 means the host does no pixel work at all.
pub enum Captured {
    Bgra(Vec<u8>),
    H264 { nal: Vec<u8>, keyframe: bool },
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
    /// Last raw frame, re-served when the captured window is static or the client is
    /// reconnecting. Without this the encoder never runs, so a requested IDR never
    /// reaches a late-joining browser and the player sits on black.
    ///
    /// Only used for BGRA. An encoded frame must never be re-sent: H.264 frames are
    /// differential, so replaying one corrupts the decoder's reference state.
    last: Option<Vec<u8>>,
    format: FrameFormat,
    keyframe: bool,
    /// The encode target the host commanded win-capture to match. Remembered so a
    /// reconnecting client is re-told immediately; a fresh win-capture process
    /// starts from its CLI defaults until this reaches it.
    target: Option<EncodeTarget>,
    /// Frames that finished `read_frame` successfully, regardless of whether the
    /// relay stage later drops them. Compared against the win-capture side's own
    /// "encoded" count, this pinpoints whether loss happens before or after the
    /// Windows→WSL socket — see `docs/OPTIMIZATION_PLAN.md` step 1.
    frames_received: u64,
    /// When the current outage started, if any — `None` while connected.
    disconnected_at: Option<Instant>,
    /// When we last asked `ensure-win-capture.sh` to relaunch it.
    last_respawn: Option<Instant>,
    /// When the last frame (of any kind, including a stale-frame no-op) was
    /// actually pulled off the socket. Drives `FRAME_STALE_AFTER`.
    last_frame_at: Instant,
    /// When the current TCP session was accepted. Hung-alive detection uses
    /// `max(last_frame_at, session_started_at)` so a real reconnect after the
    /// host waited for launch still gets a full `FRAME_STALE_AFTER` to deliver
    /// its first frame — without treating accept itself as "healthy" (that
    /// was the black-WebCodecs hang: refreshing `last_frame_at` on accept).
    session_started_at: Instant,
    /// See `HyperVBridge::ever_connected` — same startup/picker race.
    ever_connected: bool,
    /// See `HyperVBridge::pending_audio` — same demux, same cap.
    pending_audio: VecDeque<AudioFrame>,
}

impl WindowsBridge {
    /// Listen for the Windows capture client (default `0.0.0.0:9876`).
    ///
    /// Does **not** block forever on `accept`. A missing win-capture used to
    /// park the host's only async thread here, so `PeerJoined` was never
    /// drained and every friend hung on "Waiting for host offer". Bind,
    /// wait briefly, then return disconnected — `capture()` / `maybe_respawn`
    /// attach the client when it shows up.
    pub fn listen(bind: &str) -> Result<Self> {
        let listener = TcpListener::bind(bind)
            .with_context(|| format!("bind Windows capture listener on {bind}"))?;
        tracing::info!("waiting briefly for couchlink-win-capture (Windows → {bind})…");
        listener
            .set_nonblocking(true)
            .context("nonblocking accept")?;
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut stream = None;
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((s, peer)) => {
                    tracing::info!("Windows capture client connected from {peer}");
                    configure(&s)?;
                    stream = Some(s);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(e).context("accept Windows capture client");
                }
            }
        }
        if stream.is_none() {
            tracing::warn!(
                "win-capture not connected yet — host will register and attach capture when ready"
            );
        }
        let disconnected_at = stream.is_none().then(Instant::now);
        let ever_connected = stream.is_some();
        let now = Instant::now();
        Ok(Self {
            listener,
            stream,
            width: 0,
            height: 0,
            buf: Vec::new(),
            pending: None,
            last: None,
            format: FrameFormat::Bgra,
            keyframe: false,
            target: None,
            frames_received: 0,
            disconnected_at,
            last_respawn: None,
            last_frame_at: now,
            session_started_at: now,
            ever_connected,
            pending_audio: VecDeque::new(),
        })
    }

    /// Tell the Windows encoder to match this target. Safe before a format is seen:
    /// the command is round-tripped on reconnect too, so it is not lost.
    pub fn set_target(&mut self, target: EncodeTarget) {
        self.target = Some(target);
        if let Some(stream) = self.stream.as_mut() {
            if let Err(e) = write_set_target(stream, target) {
                tracing::warn!("could not command encode target to win-capture: {e}");
            } else {
                tracing::info!(
                    "commanded win-capture encode target {}x{}@{} ({} kbps)",
                    target.width,
                    target.height,
                    target.fps,
                    target.bitrate_kbps
                );
            }
        }
    }

    /// Re-send the last commanded target (e.g. after player join) so a race
    /// between host connect and win-capture's command reader cannot leave the
    /// encoder at CLI defaults (120 fps) while the relay paces at 96.
    pub fn reassert_target(&mut self) {
        if let Some(target) = self.target {
            if let Some(stream) = self.stream.as_mut() {
                if let Err(e) = write_set_target(stream, target) {
                    tracing::warn!("could not re-command encode target to win-capture: {e}");
                }
            }
        }
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
            Ok((mut stream, peer)) => {
                if configure(&stream).is_err() {
                    return false;
                }
                tracing::info!("Windows capture client reconnected from {peer}");
                // A fresh win-capture process starts at its CLI defaults; re-assert
                // the preset before it encodes its first frame.
                if let Some(target) = self.target {
                    if let Err(e) = write_set_target(&mut stream, target) {
                        tracing::warn!("could not re-command encode target after reconnect: {e}");
                    }
                }
                self.stream = Some(stream);
                self.ever_connected = true;
                // Grace for first frame on this socket — do not touch
                // last_frame_at / disconnected_at (hung writer must still trip).
                self.session_started_at = Instant::now();
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
        // See `HyperVBridge::read_frame` — win-capture multiplexes `CLA1`
        // audio into this same stream; demux and stash it, bounded so an
        // audio flood cannot wedge the video read loop.
        let mut audio_out: Vec<AudioFrame> = Vec::new();
        let info = loop {
            if audio_out.len() >= 16 {
                break None;
            }
            stream.set_read_timeout(Some(poll))?;
            let mut magic = [0u8; 4];
            let mut got = 0;
            while got < 4 {
                match stream.read(&mut magic[got..]) {
                    Ok(0) => bail!("Windows capture client disconnected"),
                    Ok(n) => got += n,
                    // Only a timeout *before any byte of the frame* is safe to shrug off.
                    Err(ref e) if is_timeout(e) && got == 0 => {
                        if !audio_out.is_empty() {
                            self.pending_audio.extend(audio_out);
                            self.drop_excess_pending_audio();
                        }
                        return Ok(None);
                    }
                    Err(e) => return Err(e).context("frame magic"),
                }
            }
            if &magic == AUDIO_MAGIC {
                stream.set_read_timeout(Some(FRAME_BODY_TIMEOUT))?;
                let af = read_audio_body(stream).context("audio body")?;
                audio_out.push(af);
                continue;
            }
            if &magic != FRAME_MAGIC {
                bail!("bad frame magic {magic:?} — capture stream desynchronized");
            }
            stream.set_read_timeout(Some(FRAME_BODY_TIMEOUT))?;
            break Some(read_frame_body_sync(stream, &mut self.buf)?);
        };
        if !audio_out.is_empty() {
            self.pending_audio.extend(audio_out);
            self.drop_excess_pending_audio();
        }
        let Some(info) = info else {
            return Ok(None);
        };
        if self.format != info.format {
            tracing::info!(
                "capture stream format is now {:?} ({})",
                info.format,
                match info.format {
                    FrameFormat::H264 => "GPU-encoded on Windows, host relays only",
                    FrameFormat::Bgra => "raw pixels, host encodes",
                }
            );
            self.format = info.format;
            self.last = None;
        }
        self.keyframe = info.keyframe;
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
        self.frames_received += 1;
        Ok(Some(info))
    }

    /// Drain the received-frame counter since the last call. Pair with
    /// win-capture's own "encoded" log to see whether frames are lost before
    /// or after the Windows→WSL socket.
    pub fn take_received(&mut self) -> u64 {
        std::mem::take(&mut self.frames_received)
    }

    /// Read the next frame.
    ///
    /// For raw pixels, skip to the newest one: the socket buffers whole frames when
    /// the encoder falls behind and consuming them in order would add permanent,
    /// growing latency.
    ///
    /// For H.264 every frame must be delivered in order. P-frames reference the
    /// frames before them, so discarding one corrupts the decoder until the next
    /// keyframe — up to IDR_INTERVAL of stutter for a few bytes saved.
    fn latest_frame(&mut self) -> Result<bool> {
        if self.read_frame(IDLE_POLL)?.is_none() {
            return Ok(false);
        }
        if self.format == FrameFormat::H264 {
            return Ok(true);
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

    /// Ask the Windows encoder for a keyframe. No-op on the raw path, where the host
    /// controls its own encoder.
    pub fn request_idr(&mut self) {
        if self.format != FrameFormat::H264 {
            return;
        }
        if let Some(stream) = self.stream.as_mut() {
            if let Err(e) = stream.write_all(&[REQUEST_IDR]) {
                tracing::warn!("could not request IDR from Windows encoder: {e}");
            }
        }
    }

    pub fn write_expedite(&mut self) {
        if self.format != FrameFormat::H264 {
            return;
        }
        if let Some(stream) = self.stream.as_mut() {
            let _ = stream.write_all(&[EXPEDITE]);
        }
    }

    pub fn format(&self) -> FrameFormat {
        self.format
    }

    /// Drain one buffered `CLA1` Opus frame, if `read_frame` demuxed any out
    /// of the video stream since the last call. Does not itself touch the
    /// socket — video's own poll cadence is what pulls bytes off the wire.
    pub fn try_take_audio(&mut self) -> Option<Vec<u8>> {
        self.pending_audio.pop_front().map(|af| af.opus)
    }

    fn drop_excess_pending_audio(&mut self) {
        while self.pending_audio.len() > MAX_PENDING_AUDIO {
            self.pending_audio.pop_front();
        }
    }

    /// Throw away everything already queued and resynchronise.
    ///
    /// The host does not read this socket until a player connects, so by then a
    /// backlog of encoded frames is waiting. Relaying it in order is faithful and
    /// permanently late; the viewer wants what is on screen *now*. Discarding the
    /// backlog would normally corrupt the decoder, so it is paired with an IDR
    /// request — the next frame is then decodable from scratch.
    pub fn resync(&mut self) {
        let mut shed = 0u32;
        while matches!(self.read_frame(DRAIN_POLL), Ok(Some(_))) {
            shed += 1;
        }
        if shed > 0 {
            tracing::info!("dropped {shed} stale capture frame(s) and asked for a keyframe");
        }
        self.last = None;
        self.request_idr();
    }

    pub fn capture(&mut self) -> Result<Option<Captured>> {
        if let Some(p) = self.pending.take() {
            self.last = Some(p.clone());
            return Ok(Some(Captured::Bgra(p)));
        }
        if self.stream.is_none() {
            if self.try_reconnect() {
                // Do NOT clear `disconnected_at` or refresh `last_frame_at`.
                // A hung win-capture keeps its TCP writer alive and will dial
                // :9876 again the moment we drop the socket, while WGC/encode
                // is still dead. Treating that accept as "healthy" reset the
                // outage clocks forever — FORCE kill never ran, friends sat on
                // black "webcodecs video" for minutes (live 2026-10-05 BO2 /
                // 2026-10-12 MK: 841× treating-as-dead, 1× relaunch).
                // Health is proven only by a real frame in the Ok(true) arm.
            } else {
                self.maybe_respawn();
            }
            return Ok(self.stale_frame());
        }
        match self.latest_frame() {
            Ok(true) => {
                self.last_frame_at = Instant::now();
                self.disconnected_at = None;
                // One copy, not two. At 720p each clone is 3.3MB and this runs on
                // every frame; the old code cloned once for `last` and again for the
                // caller.
                let frame = std::mem::take(&mut self.buf);
                if self.format == FrameFormat::H264 {
                    return Ok(Some(Captured::H264 {
                        nal: frame,
                        keyframe: self.keyframe,
                    }));
                }
                self.last = Some(frame.clone());
                Ok(Some(Captured::Bgra(frame)))
            }
            Ok(false) => {
                // Socket is still open and read cleanly returned "nothing yet" —
                // no error, so the old code never noticed a win-capture that has
                // hung (GPU/driver stall, deadlocked message pump, ...) rather
                // than crashed or disconnected. Route a long silence through the
                // same respawn path a real disconnect uses.
                //
                // Anchor to the newer of last_frame_at and session_started_at:
                // a hung writer that redials must still die after FRAME_STALE,
                // but a brand-new accept after the host waited for launch must
                // not inherit a multi-second-stale last_frame_at and get killed
                // before the first NAL lands (live 2026-10-12: encode OK, host
                // FORCE-killed mid-handshake → Connection reset loop).
                let healthy_at = self.last_frame_at.max(self.session_started_at);
                if healthy_at.elapsed() >= FRAME_STALE_AFTER {
                    tracing::warn!(
                        "no frame from win-capture in {:?} (socket still open — \
                         likely hung, not disconnected) — treating as dead",
                        healthy_at.elapsed()
                    );
                    self.stream = None;
                    self.disconnected_at.get_or_insert_with(Instant::now);
                    // Hung-alive must FORCE-kill immediately: waiting another
                    // RESPAWN_AFTER while the writer keeps reconnecting without
                    // frames just prolongs the black WebCodecs page.
                    self.force_respawn_hung();
                }
                Ok(self.stale_frame())
            }
            // A dead client must not kill the session: keep showing the last frame
            // and wait for win-capture to come back.
            Err(e) => {
                tracing::warn!("Windows capture client lost ({e:#}) — waiting for reconnect");
                self.stream = None;
                self.disconnected_at = Some(Instant::now());
                self.try_reconnect();
                Ok(self.stale_frame())
            }
        }
    }

    /// Postmortem: `docs/INCIDENT-2026-08-19-terminals-died.md`. Left alone, a
    /// dead win-capture waits forever for a reconnect that nothing triggers —
    /// this is the self-heal that closes that gap.
    fn maybe_respawn(&mut self) {
        if !self.ever_connected {
            return;
        }
        let Some(since) = self.disconnected_at else {
            return;
        };
        if since.elapsed() < RESPAWN_AFTER {
            return;
        }
        self.spawn_respawn(RESPAWN_RETRY_INTERVAL);
    }

    /// Hung (socket open, no frames): skip the disconnect grace — the writer
    /// can keep reconnecting forever without producing a frame, so waiting
    /// `RESPAWN_AFTER` never helps. Faster retry floor than crash-loop path.
    fn force_respawn_hung(&mut self) {
        if !self.ever_connected {
            return;
        }
        self.spawn_respawn(HUNG_RESPAWN_RETRY_INTERVAL);
    }

    fn spawn_respawn(&mut self, retry_floor: Duration) {
        if let Some(last) = self.last_respawn {
            if last.elapsed() < retry_floor {
                return;
            }
        }
        self.last_respawn = Some(Instant::now());
        super::respawn_windows_capture();
    }

    /// Re-serving only makes sense for raw pixels; see `last`.
    fn stale_frame(&self) -> Option<Captured> {
        if self.format == FrameFormat::H264 {
            return None;
        }
        self.last.clone().map(Captured::Bgra)
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

    /// Regression for black WebCodecs hang: a TCP accept from a hung win-capture
    /// writer must not be treated as a healthy reconnect. Previously `capture()`
    /// reset `last_frame_at` / cleared `disconnected_at` on accept, so
    /// `FRAME_STALE_AFTER` never accumulated and FORCE-kill never ran while
    /// friends stared at "webcodecs video".
    ///
    /// Accept *does* start a fresh session clock so a real post-launch
    /// reconnect gets grace for the first frame — hung writers still die once
    /// that grace elapses with no frame.
    #[test]
    fn silent_tcp_reconnect_does_not_mark_capture_healthy() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stale = Instant::now()
            .checked_sub(FRAME_STALE_AFTER + Duration::from_millis(200))
            .unwrap_or_else(Instant::now);
        let outage_start = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let mut bridge = WindowsBridge {
            listener,
            stream: None,
            width: 0,
            height: 0,
            buf: Vec::new(),
            pending: None,
            last: None,
            format: FrameFormat::H264,
            keyframe: false,
            target: None,
            frames_received: 0,
            disconnected_at: Some(outage_start),
            last_respawn: None,
            last_frame_at: stale,
            session_started_at: stale,
            ever_connected: true,
            pending_audio: VecDeque::new(),
        };

        // Writer dials but never sends a frame — the hung-alive signature.
        // Hold the socket open past FRAME_STALE_AFTER so the hang path can fire.
        let writer = std::thread::spawn(move || {
            let _stream = TcpStream::connect(addr).unwrap();
            std::thread::sleep(FRAME_STALE_AFTER + Duration::from_millis(800));
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while bridge.stream.is_none() && Instant::now() < deadline {
            let _ = bridge.capture();
        }
        assert!(bridge.stream.is_some(), "silent client never connected");
        assert!(
            bridge.disconnected_at.is_some(),
            "accept must not clear the outage clock before a frame arrives"
        );
        let last_before = bridge.last_frame_at;
        assert!(
            bridge.session_started_at > last_before,
            "accept must start a session clock (grace for first frame)"
        );

        // Immediate poll must NOT kill — session grace just started.
        let _ = bridge.capture();
        assert!(
            bridge.stream.is_some(),
            "must not kill during post-accept first-frame grace"
        );
        assert!(
            bridge.last_frame_at == last_before,
            "silent reconnect must not refresh last_frame_at"
        );

        // After FRAME_STALE_AFTER with no frame → FORCE relaunch.
        std::thread::sleep(FRAME_STALE_AFTER + Duration::from_millis(50));
        let _ = bridge.capture();
        assert!(
            bridge.last_respawn.is_some(),
            "hung-alive must FORCE-respawn once session grace elapses with no frame"
        );

        writer.join().unwrap();
    }

    /// win-capture interleaves `CLA1` Opus frames into the same socket as
    /// `CLF2` video — regression for the exact bug the stub `try_take_audio`
    /// used to have: audio must be demuxed out during the video read loop and
    /// handed back later, and video frames on either side of it must still
    /// arrive in order untouched.
    #[test]
    fn interleaved_audio_is_demuxed_without_disturbing_video() {
        use couchlink_capture_bridge::audio::{write_audio_frame, AudioFrame};
        use couchlink_capture_bridge::{write_frame_with_format, FrameFormat};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut bridge = WindowsBridge {
            listener,
            stream: None,
            width: 0,
            height: 0,
            buf: Vec::new(),
            pending: None,
            last: None,
            format: FrameFormat::Bgra,
            keyframe: false,
            target: None,
            frames_received: 0,
            disconnected_at: Some(Instant::now()),
            last_respawn: None,
            last_frame_at: Instant::now(),
            session_started_at: Instant::now(),
            ever_connected: false,
            pending_audio: VecDeque::new(),
        };

        // H.264 (the real deployment's format — GPU-encoded on Windows) is
        // used rather than Bgra: the Bgra path deliberately skips ahead to
        // the newest frame each poll (see `latest_frame`'s doc comment),
        // which would race this test's tight write timing. H.264 frames
        // must always arrive in strict order, so that ordering guarantee is
        // exactly what this test wants to exercise around the CLA1 demux.
        let writer = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            write_frame_with_format(&mut stream, 4, 4, FrameFormat::H264, true, &[0u8; 8])
                .unwrap();
            write_audio_frame(
                &mut stream,
                &AudioFrame {
                    seq: 1,
                    sample_rate: 48000,
                    channels: 2,
                    opus: vec![9; 40],
                },
            )
            .unwrap();
            write_frame_with_format(&mut stream, 4, 4, FrameFormat::H264, false, &[1u8; 8])
                .unwrap();
            stream
        });

        // First capture() picks up the pending TCP connection, not a frame.
        let deadline = Instant::now() + Duration::from_secs(2);
        while bridge.stream.is_none() && Instant::now() < deadline {
            let _ = bridge.capture();
        }
        assert!(bridge.stream.is_some(), "client never connected");

        let first = loop {
            if let Some(c) = bridge.capture().unwrap() {
                break c;
            }
            assert!(Instant::now() < deadline, "timed out waiting for first frame");
        };
        assert!(matches!(first, Captured::H264 { ref nal, keyframe: true } if nal == &vec![0u8; 8]));

        // The CLA1 in between must show up via try_take_audio, not as a
        // video frame, and must not have been silently dropped. Polling for
        // it can incidentally read frame2 too (the demux loop keeps reading
        // past audio within one `read_frame` call) — keep it rather than
        // discard it, since the point of this test is that no video frame is
        // ever lost to the audio demux.
        let mut second_frame = None;
        let audio = loop {
            if let Some(a) = bridge.try_take_audio() {
                break a;
            }
            if let Ok(Some(c)) = bridge.capture() {
                second_frame = Some(c);
            }
            assert!(Instant::now() < deadline, "timed out waiting for audio frame");
        };
        assert_eq!(audio, vec![9; 40]);

        let second = match second_frame {
            Some(c) => c,
            None => loop {
                if let Some(c) = bridge.capture().unwrap() {
                    break c;
                }
                assert!(Instant::now() < deadline, "timed out waiting for second frame");
            },
        };
        assert!(matches!(second, Captured::H264 { ref nal, keyframe: false } if nal == &vec![1u8; 8]));

        writer.join().unwrap();
    }
}
