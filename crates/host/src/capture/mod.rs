mod bridge;
mod local;

use anyhow::{bail, Context, Result};
pub use bridge::Captured;
use bridge::WindowsBridge;
use couchlink_capture_bridge::FrameFormat;
use local::ScrapCapture;

pub fn sample_avg_luma_bgra(bgra: &[u8], max_pixels: usize) -> u64 {
    local::sample_avg_luma_bgra(bgra, 0, max_pixels)
}

pub enum FrameCapture {
    Local(ScrapCapture),
    Windows(WindowsBridge),
}

impl FrameCapture {
    /// `windows_capture`: None = local display; `"auto"` / bind addr = listen for Windows client.
    pub fn open(windows_capture: Option<&str>) -> Result<Self> {
        if let Some(spec) = windows_capture.filter(|s| !s.is_empty() && *s != "0" && *s != "false") {
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
        }
    }

    pub fn height(&self) -> usize {
        match self {
            Self::Local(c) => c.height,
            Self::Windows(c) => c.height,
        }
    }

    pub fn capture(&mut self) -> Result<Option<Captured>> {
        match self {
            Self::Local(c) => Ok(c.capture_bgra()?.map(Captured::Bgra)),
            Self::Windows(c) => c.capture(),
        }
    }

    /// True when frames arrive already encoded and the host is only a relay.
    pub fn is_preencoded(&self) -> bool {
        match self {
            Self::Local(_) => false,
            Self::Windows(c) => c.format() == FrameFormat::H264,
        }
    }

    /// Ask the source for a keyframe. Only meaningful when pre-encoded.
    pub fn request_idr(&mut self) {
        if let Self::Windows(c) = self {
            c.request_idr();
        }
    }
}

fn info_log(msg: &str) {
    tracing::info!("{msg}");
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
