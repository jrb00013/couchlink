//! Client for the DualSense VHID companion (Windows named pipe and/or TCP).
//!
//! Used from native Windows hosts (pipe preferred) and from WSL/Linux hosts
//! (TCP to the Windows companion on localhost).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing::info;

use crate::map_frame::pad_frame_to_dualsense_usb_report;
use crate::vhid_proto::{encode_input, take_output_frame, VHID_TCP_PORT};
#[cfg(windows)]
use crate::vhid_proto::VHID_PIPE_NAME;
use couchlink_proto::PadFeedback;
use couchlink_proto::PadFrame;

pub struct VhidClient {
    stream: Box<dyn VhidIo>,
    rx_buf: Vec<u8>,
}

/// Candidate hosts for the companion, in priority order.
///
/// WSL matters here: the companion runs on Windows, and WSL2 has its own
/// network namespace, so Windows' loopback is not ours. The Windows side is
/// reachable at our default gateway instead.
fn vhid_tcp_hosts() -> Vec<String> {
    let mut hosts = Vec::new();
    if let Ok(h) = std::env::var("COUCHLINK_DS_VHID_HOST") {
        let h = h.trim().to_string();
        if !h.is_empty() {
            hosts.push(h);
        }
    }
    hosts.push("127.0.0.1".to_string());
    #[cfg(target_os = "linux")]
    if let Some(gw) = default_gateway_v4() {
        if !hosts.contains(&gw) {
            hosts.push(gw);
        }
    }
    hosts
}

/// Default IPv4 gateway from /proc/net/route — the Windows host under WSL2.
#[cfg(target_os = "linux")]
fn default_gateway_v4() -> Option<String> {
    parse_default_gateway(&std::fs::read_to_string("/proc/net/route").ok()?)
}

#[cfg(target_os = "linux")]
fn parse_default_gateway(routes: &str) -> Option<String> {
    for line in routes.lines().skip(1) {
        let mut cols = line.split_whitespace();
        let _iface = cols.next()?;
        let dest = cols.next()?;
        let gateway = cols.next()?;
        if dest != "00000000" {
            continue;
        }
        // Little-endian hex, e.g. "01D012AC" -> 172.18.208.1
        let raw = u32::from_str_radix(gateway, 16).ok()?;
        if raw == 0 {
            continue;
        }
        let o = raw.to_le_bytes();
        return Some(format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]));
    }
    None
}

trait VhidIo: Read + Write + Send {}
impl<T: Read + Write + Send> VhidIo for T {}

impl VhidClient {
    /// Auto: TCP localhost, then the Windows host from WSL, then named pipe.
    pub fn connect() -> Result<Self> {
        for host in vhid_tcp_hosts() {
            match Self::connect_tcp(&host, VHID_TCP_PORT) {
                Ok(c) => return Ok(c),
                Err(e) => {
                    tracing::debug!("VHID TCP {host} unavailable: {e:#}");
                }
            }
        }
        #[cfg(windows)]
        {
            if let Ok(c) = Self::connect_pipe() {
                return Ok(c);
            }
        }
        bail!(
            "DualSense VHID companion not reachable on TCP :{VHID_TCP_PORT} \
             (start couchlink-ds-vhid on Windows)"
        )
    }

    pub fn connect_tcp(host: &str, port: u16) -> Result<Self> {
        let addr = format!("{host}:{port}");
        let stream = TcpStream::connect_timeout(
            &addr
                .parse()
                .with_context(|| format!("parse DualSense VHID addr {addr}"))?,
            Duration::from_millis(200),
        )
        .with_context(|| format!("connect DualSense VHID TCP {addr}"))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_millis(1)))?;
        stream.set_nonblocking(true)?;
        info!("connected DualSense VHID TCP {addr}");
        Ok(Self {
            stream: Box::new(stream),
            rx_buf: Vec::new(),
        })
    }

    #[cfg(windows)]
    pub fn connect_pipe() -> Result<Self> {
        let stream = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(VHID_PIPE_NAME)
            .with_context(|| format!("open {VHID_PIPE_NAME}"))?;
        info!("connected DualSense VHID pipe");
        Ok(Self {
            stream: Box::new(stream),
            rx_buf: Vec::new(),
        })
    }

    pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
        let report = pad_frame_to_dualsense_usb_report(frame);
        let buf = encode_input(&report);
        self.stream.write_all(&buf)?;
        Ok(())
    }

    /// Non-blocking drain of companion→host HID output reports.
    pub fn poll_output(&mut self) -> Result<Vec<Vec<u8>>> {
        let mut tmp = [0u8; 512];
        loop {
            match self.stream.read(&mut tmp) {
                Ok(0) => bail!("DualSense VHID connection closed"),
                Ok(n) => self.rx_buf.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => break,
                Err(e) => return Err(e).context("read DualSense VHID"),
            }
        }
        let mut out = Vec::new();
        while let Some(frame) = take_output_frame(&mut self.rx_buf) {
            out.push(frame);
        }
        Ok(out)
    }

    pub fn poll_feedback(&mut self) -> Result<Vec<PadFeedback>> {
        Ok(self
            .poll_output()?
            .into_iter()
            .map(|report| PadFeedback::RawOutput { report })
            .collect())
    }
}

/// True when running under WSL (host may need the Windows TCP companion).
pub fn likely_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| v.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn parses_wsl_default_gateway() {
        // Real /proc/net/route from WSL2: gateway 01D012AC is 172.18.208.1.
        let routes = "Iface\tDestination\tGateway\n\
                      eth0\t00000000\t01D012AC\n\
                      docker0\t000011AC\t00000000\n";
        assert_eq!(
            parse_default_gateway(routes).as_deref(),
            Some("172.18.208.1")
        );
    }

    #[test]
    fn skips_non_default_and_gatewayless_routes() {
        let routes = "Iface\tDestination\tGateway\n\
                      docker0\t000011AC\t00000000\n\
                      eth0\t00000000\t00000000\n";
        assert_eq!(parse_default_gateway(routes), None);
    }

    #[test]
    fn env_override_wins_and_localhost_always_tried() {
        std::env::set_var("COUCHLINK_DS_VHID_HOST", "10.0.0.5");
        let hosts = vhid_tcp_hosts();
        std::env::remove_var("COUCHLINK_DS_VHID_HOST");
        assert_eq!(hosts.first().map(String::as_str), Some("10.0.0.5"));
        assert!(hosts.iter().any(|h| h == "127.0.0.1"));
    }
}
