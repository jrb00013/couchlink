mod bridge;
mod local;

use anyhow::{bail, Context, Result};
use bridge::WindowsBridge;
use local::ScrapCapture;

pub fn sample_avg_luma_bgra(bgra: &[u8], max_pixels: usize) -> u64 {
    local::sample_avg_luma_bgra(bgra, 0, max_pixels)
}

pub enum FrameCapture {
    Local(ScrapCapture),
    Windows(WindowsBridge),
}

impl FrameCapture {
    /// `windows_capture`: None = local display; `"auto"` = WSL → Windows IP:9876; or `host:port`.
    pub fn open(windows_capture: Option<&str>) -> Result<Self> {
        if let Some(spec) = windows_capture.filter(|s| !s.is_empty() && *s != "0" && *s != "false") {
            let addr = resolve_windows_addr(spec)?;
            info_log(&format!("using Windows desktop capture at {addr}"));
            return Ok(Self::Windows(WindowsBridge::connect(&addr)?));
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

    pub fn capture_bgra(&mut self) -> Result<Option<Vec<u8>>> {
        match self {
            Self::Local(c) => c.capture_bgra(),
            Self::Windows(c) => c.capture_bgra(),
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

pub fn resolve_windows_addr(spec: &str) -> Result<String> {
    if spec.eq_ignore_ascii_case("auto") {
        let ip = wsl_windows_host_ip().context(
            "WSL auto: could not read Windows IP from /etc/resolv.conf — set COUCHLINK_WINDOWS_CAPTURE=host:9876",
        )?;
        return Ok(format!("{ip}:9876"));
    }
    Ok(spec.to_string())
}

pub fn wsl_windows_host_ip() -> Result<String> {
    if !is_wsl() {
        bail!("not running under WSL");
    }
    let conf = std::fs::read_to_string("/etc/resolv.conf").context("read resolv.conf")?;
    for line in conf.lines() {
        let line = line.trim();
        if let Some(ip) = line.strip_prefix("nameserver ") {
            let ip = ip.trim();
            if !ip.is_empty() {
                return Ok(ip.to_string());
            }
        }
    }
    bail!("no nameserver in /etc/resolv.conf")
}
