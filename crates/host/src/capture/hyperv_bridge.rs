//! Windows desktop capture over a Hyper-V socket instead of TCP.
//!
//! The existing `bridge.rs` path is real TCP over the WSL2 virtual switch
//! (`vEthernet (WSL)`): every frame pays a NAT hop, a full IP/TCP stack, and
//! the virtual switch's own queueing — real, measurable latency for two
//! processes that happen to share the same physical RAM under Hyper-V.
//!
//! Hyper-V sockets are the primitive WSL2 already uses for its own plan9 file
//! sharing and `wslg` audio/X11 relay: a VMBus ring buffer exposed as a
//! socket, invisible to both the virtual network stack and Windows Defender
//! Firewall (it is not an IP socket, so there is no inbound-port prompt to
//! avoid — the reason the TCP path put the listener on the WSL side no longer
//! applies here).
//!
//! Roles, deliberately chosen to need no VM-GUID discovery on either side:
//! `couchlink-win-capture.exe` binds `AF_HYPERV` with `VmId = HV_GUID_WILDCARD`
//! (accept from *any* partition — no need to know this WSL VM's specific
//! GUID), and this side connects out over `AF_VSOCK` to `VMADDR_CID_HOST`
//! (2), the well-known CID WSL2 uses to mean "the Windows host". The wire
//! format on top is byte-for-byte identical to the TCP path
//! (`FRAME_MAGIC`/`read_frame_body_sync`/`write_frame_with_format`), so this
//! is purely a transport swap — nothing about frame content changes.
//!
//! Selected with `--windows-capture hyperv:<port>` (falls back to the TCP
//! bridge for a plain `host:port` spec). Additive: the TCP path is untouched
//! and remains the default, so this can be A/B'd on the same machine by
//! flipping one flag, per `docs/OPTIMIZATION_PLAN.md`'s regression discipline.

use anyhow::{bail, Context, Result};
use couchlink_capture_bridge::{
    read_frame_body_sync, write_set_target, EncodeTarget, FrameFormat, FrameInfo, FRAME_MAGIC,
    EXPEDITE, REQUEST_IDR,
};
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd};
use std::time::{Duration, Instant};
use vsock::{VsockStream, VMADDR_CID_HOST};

use super::Captured;

const IDLE_POLL: Duration = Duration::from_millis(2);
const DRAIN_POLL: Duration = Duration::from_millis(1);
const FRAME_BODY_TIMEOUT: Duration = Duration::from_secs(10);
/// Budget for one non-blocking Hyper-V connect attempt. Must stay well under
/// the host's PeerJoined handling latency — a blocking `vsock_connect` here is
/// exactly what left every friend on "Waiting for host offer" while win-capture
/// was still launching (or never launched).
const CONNECT_BUDGET: Duration = Duration::from_millis(250);
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(500);
/// See `bridge::RESPAWN_AFTER` / `docs/INCIDENT-2026-08-19-terminals-died.md`.
const RESPAWN_AFTER: Duration = Duration::from_secs(5);
const RESPAWN_RETRY_INTERVAL: Duration = Duration::from_secs(20);
/// See `bridge::FRAME_STALE_AFTER` — same fix, same reasoning, over vsock
/// instead of TCP: a hung win-capture never errors this side's read, it just
/// stops sending, so `maybe_respawn` needs a second trigger besides a socket
/// error.
const FRAME_STALE_AFTER: Duration = Duration::from_secs(4);
/// Cap on per-frame wait samples kept for p95 (SHM decision gate).
const WAIT_SAMPLE_CAP: usize = 120;

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

pub struct HyperVBridge {
    port: u32,
    stream: Option<VsockStream>,
    pub width: usize,
    pub height: usize,
    buf: Vec<u8>,
    pending: Option<Vec<u8>>,
    last: Option<Vec<u8>>,
    format: FrameFormat,
    keyframe: bool,
    target: Option<EncodeTarget>,
    frames_received: u64,
    disconnected_at: Option<Instant>,
    last_respawn: Option<Instant>,
    last_frame_at: Instant,
    /// Floor between non-blocking connect attempts so the host select loop
    /// is not spent inside `poll(250ms)` on every idle tick while win-capture
    /// is still coming up.
    last_connect_attempt: Option<Instant>,
    /// Once we have ever held a live stream, mid-session `maybe_respawn` may
    /// relaunch win-capture as `desktop` (no picker). Before that, a startup
    /// outage usually means the user is still in the capture picker — killing
    /// and replacing it with a Hidden desktop capture is exactly why the
    /// picker "never appeared".
    ever_connected: bool,
    /// Accumulated bridge wait (socket read / latest_frame) for diagnostics.
    wait_ns: u64,
    /// Number of wait samples accumulated into `wait_ns` this window.
    wait_count: u64,
    /// Accumulated buffer take/copy for diagnostics.
    copy_ns: u64,
    /// Number of copy samples accumulated into `copy_ns` this window.
    copy_count: u64,
    /// Per-frame wait samples (ms) for p95 SHM gate — capped ring.
    wait_samples_ms: Vec<f64>,
}

impl HyperVBridge {
    /// Open a Hyper-V capture handle for `port` without blocking on win-capture.
    ///
    /// A missing/slow win-capture used to spin forever in `vsock_connect` on the
    /// host's only async thread — signaling stayed registered, but `PeerJoined`
    /// was never drained, so every friend hung on "Waiting for host offer".
    /// Try once with a short budget; if win-capture is not up yet, return
    /// disconnected and let `capture()` / `maybe_respawn` attach later.
    pub fn connect(port: u32) -> Result<Self> {
        tracing::info!(
            "connecting to couchlink-win-capture over Hyper-V socket (port {port})…"
        );
        let (stream, disconnected_at) = match timed_connect(port, CONNECT_BUDGET) {
            Some(s) => {
                tracing::info!("Hyper-V capture socket connected");
                (Some(s), None)
            }
            None => {
                tracing::warn!(
                    "win-capture not reachable on Hyper-V port {port} yet — \
                     registering with signaling anyway; capture will attach when ready"
                );
                (None, Some(Instant::now()))
            }
        };
        // Do not wait for the first frame here. Frames are drained from the
        // select loop via `capture()` once we return.
        let ever_connected = stream.is_some();
        Ok(Self {
            port,
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
            last_frame_at: Instant::now(),
            last_connect_attempt: None,
            ever_connected,
            wait_ns: 0,
            wait_count: 0,
            copy_ns: 0,
            copy_count: 0,
            wait_samples_ms: Vec::with_capacity(WAIT_SAMPLE_CAP),
        })
    }

    pub fn set_target(&mut self, target: EncodeTarget) {
        self.target = Some(target);
        if let Some(stream) = self.stream.as_mut() {
            if let Err(e) = write_set_target(stream, target) {
                tracing::warn!("could not command encode target over Hyper-V socket: {e}");
            }
        }
    }

    pub fn reassert_target(&mut self) {
        if let Some(target) = self.target {
            if let Some(stream) = self.stream.as_mut() {
                if let Err(e) = write_set_target(stream, target) {
                    tracing::warn!("could not re-command encode target over Hyper-V socket: {e}");
                }
            }
        }
    }

    fn try_reconnect(&mut self) -> bool {
        if let Some(t) = self.last_connect_attempt {
            if t.elapsed() < CONNECT_RETRY_INTERVAL {
                return false;
            }
        }
        self.last_connect_attempt = Some(Instant::now());
        let Some(mut stream) = timed_connect(self.port, CONNECT_BUDGET) else {
            return false;
        };
        tracing::info!("Hyper-V capture socket reconnected");
        if let Some(target) = self.target {
            if let Err(e) = write_set_target(&mut stream, target) {
                tracing::warn!("could not re-command encode target after reconnect: {e}");
            }
        }
        self.stream = Some(stream);
        self.ever_connected = true;
        true
    }

    fn read_frame(&mut self, poll: Duration) -> Result<Option<FrameInfo>> {
        let Some(stream) = self.stream.as_ref() else {
            return Ok(None);
        };
        let mut magic = [0u8; 4];
        let mut got = 0;
        let mut deadline = DeadlineRead::new(stream, poll);
        while got < 4 {
            match deadline.read(&mut magic[got..]) {
                Ok(0) => bail!("win-capture disconnected"),
                Ok(n) => got += n,
                Err(ref e) if is_timeout(e) && got == 0 => return Ok(None),
                Err(e) => return Err(e).context("frame magic"),
            }
        }
        if &magic != FRAME_MAGIC {
            bail!("bad frame magic {magic:?} — capture stream desynchronized");
        }
        let mut deadline = DeadlineRead::new(stream, FRAME_BODY_TIMEOUT);
        let info = read_frame_body_sync(&mut deadline, &mut self.buf)?;
        if self.format != info.format {
            self.format = info.format;
            self.last = None;
        }
        self.keyframe = info.keyframe;
        if self.width != info.width as usize || self.height != info.height as usize {
            self.width = info.width as usize;
            self.height = info.height as usize;
            self.last = None;
        }
        self.frames_received += 1;
        Ok(Some(info))
    }

    pub fn take_received(&mut self) -> u64 {
        std::mem::take(&mut self.frames_received)
    }

    /// Per-frame handoff averages + wait p95 (ms), then clear the window.
    ///
    /// Returns `(wait_avg_ms, copy_avg_ms, wait_p95_ms)`. Zeros when no samples.
    /// SHM gate uses p95 (`input_photon_budget::shm_gate_trips`).
    ///
    /// Averages use the true sample counts (`wait_count` / `copy_count`), not the
    /// capped p95 ring length — dividing total ns by a truncated ring inflated
    /// mean wait ~6× and falsely screamed SHM while p95 stayed ~2ms.
    pub fn take_handoff_ms(&mut self) -> (f64, f64, f64) {
        let wait_n = std::mem::take(&mut self.wait_count).max(1) as f64;
        let copy_n = std::mem::take(&mut self.copy_count).max(1) as f64;
        let wait_avg = std::mem::take(&mut self.wait_ns) as f64 / 1_000_000.0 / wait_n;
        let copy_avg = std::mem::take(&mut self.copy_ns) as f64 / 1_000_000.0 / copy_n;
        let mut samples = std::mem::take(&mut self.wait_samples_ms);
        let p95 = if samples.is_empty() {
            0.0
        } else {
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let idx = ((samples.len() as f64 - 1.0) * 0.95).floor() as usize;
            samples[idx.min(samples.len() - 1)]
        };
        (wait_avg, copy_avg, p95)
    }

    fn latest_frame(&mut self) -> Result<bool> {
        if self.read_frame(IDLE_POLL)?.is_none() {
            return Ok(false);
        }
        if self.format == FrameFormat::H264 {
            return Ok(true);
        }
        while self.read_frame(DRAIN_POLL)?.is_some() {}
        Ok(true)
    }

    pub fn request_idr(&mut self) {
        if self.format != FrameFormat::H264 {
            return;
        }
        if let Some(stream) = self.stream.as_mut() {
            if let Err(e) = stream.write_all(&[REQUEST_IDR]) {
                tracing::warn!("could not request IDR over Hyper-V socket: {e}");
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

    pub fn resync(&mut self) {
        while matches!(self.read_frame(DRAIN_POLL), Ok(Some(_))) {}
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
                self.disconnected_at = None;
                self.last_frame_at = Instant::now();
            } else {
                self.maybe_respawn();
            }
            return Ok(self.stale_frame());
        }
        match {
            let t_wait = Instant::now();
            let got = self.latest_frame();
            let elapsed = t_wait.elapsed();
            self.wait_ns += elapsed.as_nanos() as u64;
            self.wait_count += 1;
            let wait_ms = elapsed.as_secs_f64() * 1000.0;
            if self.wait_samples_ms.len() >= WAIT_SAMPLE_CAP {
                self.wait_samples_ms.remove(0);
            }
            self.wait_samples_ms.push(wait_ms);
            got
        } {
            Ok(true) => {
                self.last_frame_at = Instant::now();
                let t_copy = Instant::now();
                let frame = std::mem::take(&mut self.buf);
                self.copy_ns += t_copy.elapsed().as_nanos() as u64;
                self.copy_count += 1;
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
                // See `bridge.rs`'s identical check: a hung (not disconnected)
                // win-capture leaves the vsock stream open and just stops
                // sending, so no read error is ever raised to trip
                // `maybe_respawn` on its own.
                if self.last_frame_at.elapsed() >= FRAME_STALE_AFTER {
                    tracing::warn!(
                        "no frame from win-capture in {:?} (Hyper-V socket still open — \
                         likely hung, not disconnected) — treating as dead",
                        self.last_frame_at.elapsed()
                    );
                    self.stream = None;
                    self.disconnected_at.get_or_insert_with(Instant::now);
                    self.maybe_respawn();
                }
                Ok(self.stale_frame())
            }
            Err(e) => {
                tracing::warn!("Hyper-V capture link lost ({e:#}) — waiting for reconnect");
                self.stream = None;
                self.disconnected_at = Some(Instant::now());
                self.try_reconnect();
                Ok(self.stale_frame())
            }
        }
    }

    fn stale_frame(&self) -> Option<Captured> {
        if self.format == FrameFormat::H264 {
            return None;
        }
        self.last.clone().map(Captured::Bgra)
    }

    /// See `bridge::WindowsBridge::maybe_respawn` /
    /// `docs/INCIDENT-2026-08-19-terminals-died.md`.
    fn maybe_respawn(&mut self) {
        if !self.ever_connected {
            // Picker / first-launch still in progress — do not taskkill it.
            return;
        }
        let Some(since) = self.disconnected_at else {
            return;
        };
        if since.elapsed() < RESPAWN_AFTER {
            return;
        }
        if let Some(last) = self.last_respawn {
            if last.elapsed() < RESPAWN_RETRY_INTERVAL {
                return;
            }
        }
        self.last_respawn = Some(Instant::now());
        super::respawn_windows_capture();
    }
}

/// Non-blocking AF_VSOCK connect with a wall-clock budget.
///
/// The vsock crate's `connect_with_cid_port` is a blocking `connect(2)`. On
/// WSL2 that call parks in `vsock_connect` for a long time when nothing is
/// listening — long enough to freeze PeerJoined handling. We open the socket
/// ourselves with `SOCK_NONBLOCK`, `poll` for completion, and hand the fd to
/// `VsockStream` only on success.
fn timed_connect(port: u32, budget: Duration) -> Option<VsockStream> {
    use nix::errno::Errno;
    use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
    use nix::sys::socket::sockopt::SocketError;
    use nix::sys::socket::{
        connect, getsockopt, socket, AddressFamily, SockFlag, SockType, VsockAddr,
    };

    let sock = socket(
        AddressFamily::Vsock,
        SockType::Stream,
        SockFlag::SOCK_CLOEXEC | SockFlag::SOCK_NONBLOCK,
        None,
    )
    .ok()?;
    let addr = VsockAddr::new(VMADDR_CID_HOST, port);
    match connect(sock.as_raw_fd(), &addr) {
        Ok(()) => {}
        Err(Errno::EINPROGRESS) => {
            let mut fds = [PollFd::new(sock.as_fd(), PollFlags::POLLOUT)];
            let timeout = PollTimeout::try_from(budget).unwrap_or(PollTimeout::ZERO);
            match poll(&mut fds, timeout) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {
                    let err = getsockopt(&sock, SocketError).ok()?;
                    if err != 0 {
                        return None;
                    }
                }
            }
        }
        Err(_) => return None,
    }
    let stream = unsafe { VsockStream::from_raw_fd(sock.into_raw_fd()) };
    configure(&stream).ok()?;
    Some(stream)
}

fn configure(stream: &VsockStream) -> Result<()> {
    // Belt-and-suspenders: harmless if honored, but on this kernel's AF_VSOCK
    // transport it silently is not — see `DeadlineRead` below for the real
    // fix. Keep it in case a future kernel/host actually implements it.
    stream
        .set_read_timeout(Some(FRAME_BODY_TIMEOUT))
        .context("set vsock read timeout")?;
    stream
        .set_nonblocking(true)
        .context("set vsock non-blocking")?;
    Ok(())
}

/// `VsockStream::set_read_timeout` sets `SO_RCVTIMEO`, which WSL2's vsock
/// transport (hv_sock/virtio-vsock) does not honor: the setsockopt call
/// succeeds, but a subsequent blocking `recv()` still waits forever for data
/// that may never arrive if win-capture hangs on the Windows side. That
/// wedges this read indefinitely — and because capture runs inline on the
/// host's single event loop (not on a separate task), one stuck read freezes
/// everything else too, including processing a player's join. Root-caused
/// live: a hung-but-still-connected win-capture left this thread parked in
/// `vsock_connectible_wait_data` for minutes with no timeout ever firing.
///
/// The fix: put the socket in non-blocking mode (`configure` above) and
/// reimplement the timeout ourselves in userspace by polling with a real
/// wall-clock deadline, exactly the semantics `read_exact` (used by
/// `read_frame_body_sync`) needs from its underlying `Read` impl.
struct DeadlineRead<'a> {
    stream: &'a VsockStream,
    deadline: Instant,
}

impl<'a> DeadlineRead<'a> {
    fn new(stream: &'a VsockStream, timeout: Duration) -> Self {
        Self {
            stream,
            deadline: Instant::now() + timeout,
        }
    }
}

impl std::io::Read for DeadlineRead<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match (&*self.stream).read(buf) {
                Ok(n) => return Ok(n),
                Err(e) if is_timeout(&e) => {
                    if Instant::now() >= self.deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "vsock read deadline exceeded (userspace timeout — SO_RCVTIMEO is not honored on this transport)",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        }
    }
}
