//! Read local DualSense via Linux hidraw — dualsensekit enumeration methodology
//! without linking libudev/hidapi (works in constrained build environments).
//!
//! hidraw is Linux-only. On other platforms the reader compiles to a stub that
//! reports no pad, so the viewer still builds and runs there with keyboard input;
//! a Windows pad would come from XInput, which is a separate backend.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(not(target_os = "linux"))]
mod stub {
    use anyhow::{bail, Result};
    use couchlink_proto::PadFrame;

    pub struct DualSenseReader;

    impl DualSenseReader {
        pub fn open_first() -> Result<Self> {
            bail!("DualSense over hidraw is Linux-only; use keyboard input on this platform")
        }
        pub fn read_frame(&mut self) -> Result<Option<PadFrame>> {
            Ok(None)
        }
        pub fn set_rumble(&mut self, _large: u8, _small: u8) -> Result<()> {
            Ok(())
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use stub::DualSenseReader;

#[cfg(target_os = "linux")]
pub use linux_impl::DualSenseReader;

#[cfg(target_os = "linux")]
mod linux_impl {
use anyhow::{bail, Context, Result};
use couchlink_pad::dualsense::{PID_DUALSENSE, PID_DUALSENSE_EDGE, SONY_VID};
use couchlink_pad::parse_input_report;
use couchlink_proto::PadFrame;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;

pub struct DualSenseReader {
    file: File,
    seq: u32,
    path: PathBuf,
}

impl DualSenseReader {
    pub fn open_first() -> Result<Self> {
        let path = find_dualsense_hidraw()?.context("no DualSense hidraw node found")?;
        let file = File::options()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        // Non-blocking reads so the WebRTC loop stays responsive.
        set_nonblocking(file.as_raw_fd())?;
        info!("opened DualSense at {}", path.display());
        Ok(Self {
            file,
            seq: 0,
            path,
        })
    }

    pub fn read_frame(&mut self) -> Result<Option<PadFrame>> {
        let mut buf = [0u8; 128];
        match self.file.read(&mut buf) {
            Ok(0) => Ok(None),
            Ok(n) => {
                let mut frame = match parse_input_report(&buf[..n]) {
                    Some(f) => f,
                    None => return Ok(None),
                };
                self.seq = self.seq.wrapping_add(1);
                frame.seq = self.seq;
                Ok(Some(frame))
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", self.path.display())),
        }
    }

    /// Best-effort USB output rumble (report 0x02) when connected over USB hidraw.
    pub fn set_rumble(&mut self, large: u8, small: u8) -> Result<()> {
        let mut buf = [0u8; 48];
        buf[0] = 0x02;
        buf[1] = 0xFF;
        buf[3] = small;
        buf[4] = large;
        let _ = self.file.write(&buf);
        Ok(())
    }
}

fn find_dualsense_hidraw() -> Result<Option<PathBuf>> {
    let Ok(entries) = fs::read_dir("/sys/class/hidraw") else {
        bail!("no /sys/class/hidraw — is this Linux?");
    };
    let mut usb = Vec::new();
    let mut bt = Vec::new();
    for ent in entries.flatten() {
        let name = ent.file_name();
        let name = name.to_string_lossy();
        let uevent = ent.path().join("device/uevent");
        let Ok(txt) = fs::read_to_string(&uevent) else {
            continue;
        };
        // HID_ID=0003:0000054C:00000CE6  (bus:vid:pid)
        let Some(id_line) = txt.lines().find(|l| l.starts_with("HID_ID=")) else {
            continue;
        };
        let parts: Vec<&str> = id_line.trim_start_matches("HID_ID=").split(':').collect();
        if parts.len() < 3 {
            continue;
        }
        let vid = u16::from_str_radix(parts[1], 16).unwrap_or(0);
        let pid = u16::from_str_radix(parts[2], 16).unwrap_or(0);
        if vid != SONY_VID || (pid != PID_DUALSENSE && pid != PID_DUALSENSE_EDGE) {
            continue;
        }
        let node = PathBuf::from(format!("/dev/{name}"));
        // Prefer USB bus (0003) over Bluetooth (0005) like dualsensekit.
        if parts[0].eq_ignore_ascii_case("0003") {
            usb.push(node);
        } else {
            bt.push(node);
        }
    }
    Ok(usb.into_iter().chain(bt).next())
}

fn set_nonblocking(fd: i32) -> Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    let flags = fcntl(fd, FcntlArg::F_GETFL).context("F_GETFL")?;
    let flags = OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK;
    fcntl(fd, FcntlArg::F_SETFL(flags)).context("F_SETFL")?;
    let _ = Duration::from_millis(1);
    Ok(())
}
}
