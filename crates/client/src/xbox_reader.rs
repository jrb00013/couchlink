//! Read a local Xbox controller via Linux hidraw, same approach as
//! `dualsense_reader.rs`: no libudev/hidapi dependency, just scan
//! `/sys/class/hidraw` for Microsoft's vendor id and a known Xbox product id.
//!
//! This is what makes Xbox controllers work as client-side pad input —
//! whoever runs `couchlink-client` (the friend, or the host operator running
//! a second local client instance) gets the same DualSense-shaped `PadFrame`
//! out of an Xbox controller that the DualSense reader produces, so the
//! virtual pad the host injects behaves identically either way. hidraw is
//! Linux-only; other platforms compile to a stub that reports no pad, same
//! as the DualSense reader.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(not(target_os = "linux"))]
mod stub {
    use anyhow::{bail, Result};
    use couchlink_proto::PadFrame;

    pub struct XboxReader;

    impl XboxReader {
        pub fn open_first() -> Result<Self> {
            bail!("Xbox controller over hidraw is Linux-only; use keyboard input on this platform")
        }
        pub fn read_frame(&mut self) -> Result<Option<PadFrame>> {
            Ok(None)
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use stub::XboxReader;

#[cfg(target_os = "linux")]
pub use linux_impl::XboxReader;

#[cfg(target_os = "linux")]
mod linux_impl {
use anyhow::{bail, Context, Result};
use couchlink_pad::parse_xbox_input_report;
use couchlink_pad::xbox::{KNOWN_PIDS, MICROSOFT_VID};
use couchlink_proto::PadFrame;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use tracing::info;

pub struct XboxReader {
    file: File,
    seq: u32,
    path: PathBuf,
}

impl XboxReader {
    pub fn open_first() -> Result<Self> {
        let path = find_xbox_hidraw()?.context("no Xbox controller hidraw node found")?;
        let file = File::options()
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        // Non-blocking reads so the WebRTC loop stays responsive.
        set_nonblocking(file.as_raw_fd())?;
        info!("opened Xbox controller at {}", path.display());
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
                let mut frame = match parse_xbox_input_report(&buf[..n]) {
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
}

fn find_xbox_hidraw() -> Result<Option<PathBuf>> {
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
        // HID_ID=0003:0000045E:000002FD  (bus:vid:pid)
        let Some(id_line) = txt.lines().find(|l| l.starts_with("HID_ID=")) else {
            continue;
        };
        let parts: Vec<&str> = id_line.trim_start_matches("HID_ID=").split(':').collect();
        if parts.len() < 3 {
            continue;
        }
        let vid = u16::from_str_radix(parts[1], 16).unwrap_or(0);
        let pid = u16::from_str_radix(parts[2], 16).unwrap_or(0);
        if vid != MICROSOFT_VID || !KNOWN_PIDS.contains(&pid) {
            continue;
        }
        let node = PathBuf::from(format!("/dev/{name}"));
        // Prefer USB bus (0003) over Bluetooth (0005), same preference the
        // DualSense reader applies.
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
    Ok(())
}
}
