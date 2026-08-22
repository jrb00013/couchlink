mod bridge;
mod local;
#[cfg(target_os = "linux")]
mod cursor_x11;
#[cfg(target_os = "linux")]
mod hyperv_bridge;

use anyhow::{bail, Context, Result};
pub use bridge::Captured;
use bridge::WindowsBridge;
use couchlink_capture_bridge::{EncodeTarget, FrameFormat};
#[cfg(target_os = "linux")]
use hyperv_bridge::HyperVBridge;
use local::ScrapCapture;

pub fn sample_avg_luma_bgra(bgra: &[u8], max_pixels: usize) -> u64 {
    local::sample_avg_luma_bgra(bgra, 0, max_pixels)
}

pub enum FrameCapture {
    Local(ScrapCapture),
    Windows(WindowsBridge),
    #[cfg(target_os = "linux")]
    HyperV(HyperVBridge),
}

impl FrameCapture {
    /// `windows_capture`: None = local display; `"auto"` / bind addr = listen for Windows
    /// client over TCP; `"hyperv:<port>"` or `"hyperv:<port>:<vm-id>"` = connect out over
    /// a Hyper-V socket instead — the host only needs the port (the VmId is win-capture's
    /// own bind parameter, passed on its `--connect`, not this side's) — see
    /// `hyperv_bridge.rs` for why that skips the WSL2 virtual network stack entirely.
    pub fn open(windows_capture: Option<&str>) -> Result<Self> {
        if let Some(spec) = windows_capture.filter(|s| !s.is_empty() && *s != "0" && *s != "false") {
            #[cfg(target_os = "linux")]
            if let Some(rest) = spec.strip_prefix("hyperv:") {
                let port_str = rest.split(':').next().unwrap_or(rest);
                let port: u32 = port_str
                    .parse()
                    .with_context(|| format!("bad hyperv port {port_str:?} (expected a number)"))?;
                info_log(&format!("Windows desktop capture over Hyper-V socket (port {port})"));
                return Ok(Self::HyperV(HyperVBridge::connect(port)?));
            }
            let bind = resolve_listen_addr(spec)?;
            info_log(&format!("Windows desktop capture listening on {bind}"));
            return Ok(Self::Windows(WindowsBridge::listen(&bind)?));
        }
        Ok(Self::Local(ScrapCapture::primary()?))
    }

    pub fn width(&self) -> usize {
        match self {
            Self::Local(c) => c.width,
            Self::Windows(c) => c.width,
            #[cfg(target_os = "linux")]
            Self::HyperV(c) => c.width,
        }
    }

    pub fn height(&self) -> usize {
        match self {
            Self::Local(c) => c.height,
            Self::Windows(c) => c.height,
            #[cfg(target_os = "linux")]
            Self::HyperV(c) => c.height,
        }
    }

    pub fn capture(&mut self) -> Result<Option<Captured>> {
        match self {
            Self::Local(c) => Ok(c.capture_bgra()?.map(Captured::Bgra)),
            Self::Windows(c) => c.capture(),
            #[cfg(target_os = "linux")]
            Self::HyperV(c) => c.capture(),
        }
    }

    /// True when frames arrive already encoded and the host is only a relay.
    pub fn is_preencoded(&self) -> bool {
        match self {
            Self::Local(_) => false,
            Self::Windows(c) => c.format() == FrameFormat::H264,
            #[cfg(target_os = "linux")]
            Self::HyperV(c) => c.format() == FrameFormat::H264,
        }
    }

    /// Ask the source for a keyframe. Only meaningful when pre-encoded.
    pub fn request_idr(&mut self) {
        match self {
            Self::Windows(c) => c.request_idr(),
            #[cfg(target_os = "linux")]
            Self::HyperV(c) => c.request_idr(),
            Self::Local(_) => {}
        }
    }

    /// Command the Windows encoder to match the stream target. No-op on the local
    /// path, where the host's own encoder already uses the preset directly.
    pub fn set_target(&mut self, target: EncodeTarget) {
        match self {
            Self::Windows(c) => c.set_target(target),
            #[cfg(target_os = "linux")]
            Self::HyperV(c) => c.set_target(target),
            Self::Local(_) => {}
        }
    }

    /// Discard anything already buffered so the stream starts from *now*.
    pub fn resync(&mut self) {
        match self {
            Self::Windows(c) => c.resync(),
            #[cfg(target_os = "linux")]
            Self::HyperV(c) => c.resync(),
            Self::Local(_) => {}
        }
    }

    /// Frames received since the last call. Always 0 on the local path, which
    /// has no socket hop to lose frames over.
    pub fn take_received(&mut self) -> u64 {
        match self {
            Self::Local(_) => 0,
            Self::Windows(c) => c.take_received(),
            #[cfg(target_os = "linux")]
            Self::HyperV(c) => c.take_received(),
        }
    }
}

fn info_log(msg: &str) {
    tracing::info!("{msg}");
}

/// Postmortem: `docs/INCIDENT-2026-08-19-terminals-died.md`. win-capture died
/// alongside a crashed terminal and nothing ever relaunched it — the host
/// kept running and the player just saw a frozen picture until they rejoined
/// (which didn't even help, since capture was still down). `bridge.rs` and
/// `hyperv_bridge.rs` call this once the Windows side has been gone longer
/// than a moment, so a dead win-capture heals itself instead of waiting on a
/// reconnect that nothing was ever going to trigger.
///
/// Fire-and-forget: `ensure-win-capture.sh` launches PowerShell, which can
/// take real time, and this runs from the capture poll loop — blocking here
/// would turn a capture outage into a frame-loop stall too. Repo root comes
/// from `COUCHLINK_ROOT` (set by `start-host.sh`), same lookup
/// `emulator_pad.rs` already uses for its own best-effort script re-runs.
pub(crate) fn respawn_windows_capture() {
    let Ok(root) = std::env::var("COUCHLINK_ROOT") else {
        tracing::warn!("win-capture link down and COUCHLINK_ROOT unset — cannot self-heal");
        return;
    };
    let script = std::path::Path::new(&root).join("scripts/ensure-win-capture.sh");
    if !script.is_file() {
        return;
    }
    tracing::warn!("win-capture link has been down too long — relaunching it");
    if let Err(e) = std::process::Command::new("bash")
        .arg(&script)
        .current_dir(&root)
        // `ensure-win-capture.sh` defaults to the interactive picker, which
        // `run.sh` launches with `WindowStyle Normal` specifically so it can
        // steal focus and be clicked (see AllowSetForegroundWindow in
        // win_capture.rs). That is correct for the user-driven first launch,
        // but this is an unattended mid-session respawn — re-popping a
        // foreground picker dialog here yanks focus off the game (and the
        // remote player's controller input with it, since XInput delivery
        // depends on whichever window currently has it) for no reason: there
        // is nothing to click, nobody watching for it, and it just steals
        // focus until it times out or falls back on its own. Force a
        // non-interactive capture source instead, unless the caller already
        // pinned a specific one.
        .env(
            "COUCHLINK_CAPTURE_SOURCE",
            std::env::var("COUCHLINK_CAPTURE_SOURCE")
                .ok()
                .filter(|s| s != "picker" && !s.is_empty())
                .unwrap_or_else(|| "desktop".to_string()),
        )
        // Respawn is unattended — don't cargo-build on every 20s retry.
        // That was opening a blue PowerShell even when the exe was already there.
        .env("COUCHLINK_SKIP_WIN_CAPTURE_BUILD", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        tracing::warn!("could not relaunch win-capture: {e}");
    }
}

pub fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| v.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

pub fn resolve_listen_addr(spec: &str) -> Result<String> {
    if spec.eq_ignore_ascii_case("auto") {
        return Ok("0.0.0.0:9876".into());
    }
    Ok(spec.to_string())
}

/// Best-effort Windows host IP as seen from WSL2 (NAT gateway, not mirrored DNS).
#[allow(dead_code)]
pub fn wsl_windows_host_ip() -> Result<String> {
    if !is_wsl() {
        bail!("not running under WSL");
    }
    // Prefer default route gateway (e.g. 172.18.208.1). Mirrored WSL often puts
    // 10.255.255.254 in resolv.conf which is NOT the capture listener.
    if let Ok(route) = std::fs::read_to_string("/proc/net/route") {
        for line in route.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() >= 3 && cols[1] == "00000000" {
                if let Ok(raw) = u32::from_str_radix(cols[2], 16) {
                    let ip = std::net::Ipv4Addr::from(raw.to_le_bytes());
                    if !ip.is_unspecified() {
                        return Ok(ip.to_string());
                    }
                }
            }
        }
    }
    let conf = std::fs::read_to_string("/etc/resolv.conf").context("read resolv.conf")?;
    for line in conf.lines() {
        let line = line.trim();
        if let Some(ip) = line.strip_prefix("nameserver ") {
            let ip = ip.trim();
            // Skip WSL mirrored stub resolver.
            if ip == "10.255.255.254" || ip.is_empty() {
                continue;
            }
            return Ok(ip.to_string());
        }
    }
    bail!("could not determine Windows host IP — set COUCHLINK_WINDOWS_CAPTURE=host:9876")
}
