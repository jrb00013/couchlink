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

trait VhidIo: Read + Write + Send {}
impl<T: Read + Write + Send> VhidIo for T {}

impl VhidClient {
    /// Auto: TCP localhost (WSL + native), then Windows named pipe.
    pub fn connect() -> Result<Self> {
        match Self::connect_tcp("127.0.0.1", VHID_TCP_PORT) {
            Ok(c) => return Ok(c),
            Err(e) => {
                tracing::debug!("VHID TCP unavailable: {e:#}");
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
