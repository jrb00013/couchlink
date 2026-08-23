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

    pub fn write_expedite(&mut self) {
        match self {
            Self::Windows(c) => c.write_expedite(),
            #[cfg(target_os = "linux")]
            Self::HyperV(c) => c.write_expedite(),
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

    /// Hyper-V handoff: `(wait_avg_ms, copy_avg_ms, wait_p95_ms)`. Zeros elsewhere.
    pub fn take_handoff_ms(&mut self) -> (f64, f64, f64) {
        match self {
            #[cfg(target_os = "linux")]
            Self::HyperV(c) => c.take_handoff_ms(),
            _ => (0.0, 0.0, 0.0),
        }
    }
}

/// Which capture IPC transport was requested (`COUCHLINK_CAPTURE_IPC`).
///
/// SHM is parseable so the gate can be A/B'd, but the body is not implemented
/// until live `wait_p95` trips `shm_gate_trips` — requesting `shm` falls back
/// to Hyper-V with a warning (see `resolve_capture_ipc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureIpc {
    HyperV,
    Tcp,
    Shm,
}

/// Parse `COUCHLINK_CAPTURE_IPC` / explicit ipc name. Case-insensitive.
pub fn parse_capture_ipc(s: &str) -> Result<CaptureIpc, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "shm" => Ok(CaptureIpc::Shm),
        "hyperv" => Ok(CaptureIpc::HyperV),
        "tcp" => Ok(CaptureIpc::Tcp),
        other => Err(format!(
            "unknown capture ipc {other:?} — expected shm|hyperv|tcp"
        )),
    }
}

/// Resolve requested IPC. SHM is not built yet — fall back to Hyper-V until
/// live measurements trip the gate (MATH-4 / AMAZE-5).
pub fn resolve_capture_ipc(requested: CaptureIpc) -> CaptureIpc {
    match requested {
        CaptureIpc::Shm => CaptureIpc::HyperV,
        other => other,
    }
}

/// Log-friendly name.
pub fn capture_ipc_label(ipc: CaptureIpc) -> &'static str {
    match ipc {
        CaptureIpc::HyperV => "hyperv",
        CaptureIpc::Tcp => "tcp",
        CaptureIpc::Shm => "shm",
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
    let mut cmd = respawn_command(
        std::path::Path::new(&root),
        &script,
        std::env::var("COUCHLINK_CAPTURE_SOURCE").ok(),
    );
    if let Err(e) = cmd.spawn() {
        tracing::warn!("could not relaunch win-capture: {e}");
    }
}

/// Build (but do not spawn) the unattended win-capture relaunch. Split from
/// `respawn_windows_capture` so tests can assert on the environment without
/// actually launching PowerShell.
///
/// The capture source is *not* downgraded here: when the picked window closes,
/// windows-capture halts the session (`on_closed` → WM_QUIT → the process
/// exits), and this respawn is what brings capture back — with the picker, so
/// the selector pops back up and the host picks the next window instead of the
/// stream silently returning as whole-desktop capture. A source pinned via
/// `COUCHLINK_CAPTURE_SOURCE` (desktop / window / …) is still honoured exactly.
fn respawn_command(root: &std::path::Path, script: &std::path::Path, configured: Option<String>) -> std::process::Command {
    let mut cmd = std::process::Command::new("bash");
    cmd.arg(script)
        .current_dir(root)
        .env(
            "COUCHLINK_CAPTURE_SOURCE",
            respawn_capture_source(configured),
        )
        // Respawn is unattended — don't cargo-build on every 20s retry.
        // That was opening a blue PowerShell even when the exe was already there.
        .env("COUCHLINK_SKIP_WIN_CAPTURE_BUILD", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd
}

/// Which capture source a respawned win-capture should use.
///
/// `ensure-win-capture.sh` defaults to the interactive picker; an old version
/// of this path overrode that to `desktop`, reasoning that re-popping a
/// foreground picker mid-session steals focus for no reason. But the reason
/// *is* the point now: the outage this fires on is usually the picked window
/// closing — there is no game window left to steal focus *from*, and silently
/// falling back to desktop capture replaced "the game I picked" with "the
/// whole desktop" without telling anyone why. So an explicitly pinned source
/// passes through untouched. When `COUCHLINK_CAPTURE_WINDOW` is set, respawn
/// retries that title (no picker). Otherwise unpinned sessions reopen the picker.
pub(crate) fn respawn_capture_source(configured: Option<String>) -> String {
    respawn_capture_source_inner(configured, window_capture_pinned())
}

fn respawn_capture_source_inner(configured: Option<String>, pinned_window: bool) -> String {
    match configured {
        Some(s) if !s.is_empty() => s,
        _ if pinned_window => "window".to_string(),
        _ => "picker".to_string(),
    }
}

fn window_capture_pinned() -> bool {
    std::env::var("COUCHLINK_CAPTURE_WINDOW")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the feature: a session that started from the picker
    /// (no pinned source — the default) must respawn with the picker again, so
    /// closing the selected window pops the capture selector back up instead
    /// of silently falling back to desktop capture.
    #[test]
    fn pinned_window_title_respawns_window_not_picker() {
        assert_eq!(
            respawn_capture_source_inner(None, true),
            "window"
        );
    }

    #[test]
    fn unpinned_respawn_reopens_the_picker() {
        assert_eq!(respawn_capture_source_inner(None, false), "picker");
    }

    #[test]
    fn empty_capture_source_counts_as_unpinned() {
        assert_eq!(respawn_capture_source_inner(Some(String::new()), false), "picker");
    }

    #[test]
    fn explicit_picker_stays_picker() {
        assert_eq!(respawn_capture_source_inner(Some("picker".into()), false), "picker");
    }

    /// The whole point of the feature: a session that started from the picker
    /// (no pinned source — the default) must respawn with the picker again, so
    /// closing the selected window pops the capture selector back up instead
    /// of silently falling back to desktop capture.
    /// Someone who pinned `COUCHLINK_CAPTURE_SOURCE=desktop` (or window mode)
    /// asked for that source explicitly — an unattended respawn must not turn
    /// it into a dialog.
    #[test]
    fn pinned_noninteractive_sources_are_honoured() {
        assert_eq!(respawn_capture_source(Some("desktop".into())), "desktop");
        assert_eq!(respawn_capture_source(Some("window".into())), "window");
    }

    fn env_of(cmd: &std::process::Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == std::ffi::OsStr::new(key))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
    }

    #[test]
    fn respawn_command_carries_explicit_picker_source() {
        let cmd = respawn_command(
            std::path::Path::new("/tmp"),
            std::path::Path::new("/tmp/e.sh"),
            Some("picker".into()),
        );
        assert_eq!(
            env_of(&cmd, "COUCHLINK_CAPTURE_SOURCE").as_deref(),
            Some("picker")
        );
    }

    #[test]
    fn respawn_command_honours_a_pinned_source() {
        let cmd = respawn_command(
            std::path::Path::new("/tmp"),
            std::path::Path::new("/tmp/e.sh"),
            Some("desktop".into()),
        );
        assert_eq!(
            env_of(&cmd, "COUCHLINK_CAPTURE_SOURCE").as_deref(),
            Some("desktop")
        );
    }

    /// Respawns fire from the frame loop every RESPAWN_RETRY_INTERVAL — they
    /// must run the relaunch script directly (no rebuild step in between) and
    /// never inherit this process's stdio.
    #[test]
    fn respawn_command_skips_build_and_is_fully_detached() {
        let cmd = respawn_command(std::path::Path::new("/tmp"), std::path::Path::new("/tmp/e.sh"), None);
        assert_eq!(
            env_of(&cmd, "COUCHLINK_SKIP_WIN_CAPTURE_BUILD").as_deref(),
            Some("1")
        );
        assert_eq!(cmd.get_program(), std::ffi::OsStr::new("bash"));
        let args: Vec<std::ffi::OsString> = cmd.get_args().map(|a| a.to_os_string()).collect();
        assert_eq!(args, vec![std::ffi::OsString::from("/tmp/e.sh")]);
    }

    #[test]
    fn parse_capture_ipc_accepts_shm_hyperv_tcp() {
        assert_eq!(parse_capture_ipc("shm"), Ok(CaptureIpc::Shm));
        assert_eq!(parse_capture_ipc("hyperv"), Ok(CaptureIpc::HyperV));
        assert_eq!(parse_capture_ipc("TCP"), Ok(CaptureIpc::Tcp));
        assert!(parse_capture_ipc("nope").is_err());
    }

    #[test]
    fn resolve_capture_ipc_falls_back_shm_until_gate() {
        // SHM body not implemented — requesting it must not crash; Hyper-V stays live.
        assert_eq!(resolve_capture_ipc(CaptureIpc::Shm), CaptureIpc::HyperV);
        assert_eq!(resolve_capture_ipc(CaptureIpc::HyperV), CaptureIpc::HyperV);
        assert_eq!(resolve_capture_ipc(CaptureIpc::Tcp), CaptureIpc::Tcp);
    }
}
