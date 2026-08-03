//! Linux `/dev/uhid` DualSense so `hid-playstation` can deliver OUTPUT reports.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;

use anyhow::{Context, Result};
use couchlink_proto::{PadFeedback, PadFrame};
use tracing::info;

use crate::dualsense::{DUALSENSE_HID_REPORT_DESCRIPTOR, USB_REPORT_LEN};
use crate::map_frame::pad_frame_to_dualsense_usb_report;

/// Minimal identity bits needed to create the UHID device (avoids a module cycle).
pub struct UhidIdentity<'a> {
    pub name: &'a str,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
    pub as_bluetooth: bool,
}

const UHID_DESTROY: u32 = 1;
const UHID_OUTPUT: u32 = 6;
const UHID_GET_REPORT: u32 = 9;
const UHID_GET_REPORT_REPLY: u32 = 10;
const UHID_CREATE2: u32 = 11;
const UHID_INPUT2: u32 = 12;
const UHID_SET_REPORT: u32 = 13;
const UHID_SET_REPORT_REPLY: u32 = 14;

const BUS_USB: u16 = 0x03;
const BUS_BLUETOOTH: u16 = 0x05;

/// Offset of `uhid_event.u.create2.rd_data` from start of `uhid_event`.
const CREATE2_RD_DATA_OFF: usize = 4 + 128 + 64 + 64 + 2 + 2 + 4 + 4 + 4 + 4;

pub struct LinuxUhid {
    file: std::fs::File,
    pending_outputs: Vec<Vec<u8>>,
}

impl LinuxUhid {
    pub fn create(id: &UhidIdentity<'_>) -> Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/uhid")
            .context("open /dev/uhid (need uhid module + permissions)")?;

        let rd = DUALSENSE_HID_REPORT_DESCRIPTOR;
        let mut ev = vec![0u8; CREATE2_RD_DATA_OFF + rd.len()];
        ev[0..4].copy_from_slice(&UHID_CREATE2.to_le_bytes());
        let name = id.name.as_bytes();
        let n = name.len().min(127);
        ev[4..4 + n].copy_from_slice(&name[..n]);
        let phys = b"couchlink-uhid";
        ev[4 + 128..4 + 128 + phys.len()].copy_from_slice(phys);
        let uniq = b"couchlink-p2";
        ev[4 + 128 + 64..4 + 128 + 64 + uniq.len()].copy_from_slice(uniq);
        let mut o = 4 + 128 + 64 + 64;
        ev[o..o + 2].copy_from_slice(&(rd.len() as u16).to_le_bytes());
        o += 2;
        let bus = if id.as_bluetooth {
            BUS_BLUETOOTH
        } else {
            BUS_USB
        };
        ev[o..o + 2].copy_from_slice(&bus.to_le_bytes());
        o += 2;
        ev[o..o + 4].copy_from_slice(&(id.vendor as u32).to_le_bytes());
        o += 4;
        ev[o..o + 4].copy_from_slice(&(id.product as u32).to_le_bytes());
        o += 4;
        ev[o..o + 4].copy_from_slice(&(id.version as u32).to_le_bytes());
        o += 4;
        ev[o..o + 4].copy_from_slice(&0u32.to_le_bytes());
        o += 4;
        debug_assert_eq!(o, CREATE2_RD_DATA_OFF);
        ev[CREATE2_RD_DATA_OFF..].copy_from_slice(rd);

        file.write_all(&ev).context("UHID_CREATE2")?;
        // Drain START/OPEN so the device is live.
        let mut pad = Self {
            file,
            pending_outputs: Vec::new(),
        };
        pad.pump_events()?;
        info!(
            "virtual pad ready via /dev/uhid: '{}' vid={:04x} pid={:04x}",
            id.name, id.vendor, id.product
        );
        Ok(pad)
    }

    pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
        self.pump_events()?;
        let report = pad_frame_to_dualsense_usb_report(frame);
        // type(4) + size(2) + data
        let mut ev = vec![0u8; 4 + 2 + USB_REPORT_LEN];
        ev[0..4].copy_from_slice(&UHID_INPUT2.to_le_bytes());
        ev[4..6].copy_from_slice(&(USB_REPORT_LEN as u16).to_le_bytes());
        ev[6..].copy_from_slice(&report);
        self.file.write_all(&ev).context("UHID_INPUT2")?;
        Ok(())
    }

    pub fn poll_feedback(&mut self) -> Result<Vec<PadFeedback>> {
        self.pump_events()?;
        Ok(self
            .pending_outputs
            .drain(..)
            .map(|report| PadFeedback::RawOutput { report })
            .collect())
    }

    fn pump_events(&mut self) -> Result<()> {
        // Full `struct uhid_event` (~4KB+); kernel may write short — zero-extend.
        let mut buf = vec![0u8; 4 + 4096 + 64];
        loop {
            match self.file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if n < buf.len() {
                        buf[n..].fill(0);
                    }
                    self.handle_event(&buf)?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e).context("read /dev/uhid"),
            }
        }
        Ok(())
    }

    fn handle_event(&mut self, raw: &[u8]) -> Result<()> {
        if raw.len() < 4 {
            return Ok(());
        }
        let ty = u32::from_le_bytes(raw[0..4].try_into().unwrap());
        match ty {
            UHID_OUTPUT => {
                // uhid_output_req: data[4096] + size u16 + rtype u8
                if raw.len() < 4 + 4096 + 2 {
                    return Ok(());
                }
                let size =
                    u16::from_le_bytes(raw[4 + 4096..4 + 4096 + 2].try_into().unwrap()) as usize;
                if size == 0 {
                    return Ok(());
                }
                let end = size.min(4096);
                self.pending_outputs.push(raw[4..4 + end].to_vec());
            }
            UHID_GET_REPORT => {
                // Reply empty success so enumeration does not hang.
                if raw.len() < 4 + 4 + 1 + 1 {
                    return Ok(());
                }
                let id = u32::from_le_bytes(raw[4..8].try_into().unwrap());
                let rnum = raw[8];
                let mut reply = vec![0u8; 4 + 4 + 2 + 2 + 64];
                reply[0..4].copy_from_slice(&UHID_GET_REPORT_REPLY.to_le_bytes());
                reply[4..8].copy_from_slice(&id.to_le_bytes());
                reply[8..10].copy_from_slice(&0u16.to_le_bytes()); // err
                reply[10..12].copy_from_slice(&1u16.to_le_bytes()); // size
                reply[12] = rnum; // report id echo
                let _ = self.file.write_all(&reply);
            }
            UHID_SET_REPORT => {
                if raw.len() < 4 + 4 {
                    return Ok(());
                }
                let id = u32::from_le_bytes(raw[4..8].try_into().unwrap());
                let mut reply = [0u8; 4 + 4 + 2];
                reply[0..4].copy_from_slice(&UHID_SET_REPORT_REPLY.to_le_bytes());
                reply[4..8].copy_from_slice(&id.to_le_bytes());
                // err = 0
                let _ = self.file.write_all(&reply);
            }
            _ => {}
        }
        Ok(())
    }
}

impl Drop for LinuxUhid {
    fn drop(&mut self) {
        let mut ev = [0u8; 4];
        ev.copy_from_slice(&UHID_DESTROY.to_le_bytes());
        let _ = self.file.write_all(&ev);
    }
}

/// True when `/dev/uhid` exists and is writable enough to try.
pub fn uhid_available() -> bool {
    std::path::Path::new("/dev/uhid").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create2_rd_data_offset_matches_kernel_layout() {
        // type(4) + name(128) + phys(64) + uniq(64) + rd_size(2) + bus(2)
        // + vendor(4) + product(4) + version(4) + country(4)
        assert_eq!(CREATE2_RD_DATA_OFF, 4 + 128 + 64 + 64 + 2 + 2 + 4 + 4 + 4 + 4);
        assert!(!DUALSENSE_HID_REPORT_DESCRIPTOR.is_empty());
        assert_eq!(DUALSENSE_HID_REPORT_DESCRIPTOR[0], 0x05);
        assert_eq!(*DUALSENSE_HID_REPORT_DESCRIPTOR.last().unwrap(), 0xC0);
    }

    #[test]
    fn descriptor_fits_hid_max() {
        assert!(DUALSENSE_HID_REPORT_DESCRIPTOR.len() < 4096);
        assert_eq!(DUALSENSE_HID_REPORT_DESCRIPTOR.len(), 273);
    }
}

// libc O_NONBLOCK without pulling a full libc dep on non-linux — use nix.
mod libc {
    pub const O_NONBLOCK: i32 = nix::libc::O_NONBLOCK;
}
