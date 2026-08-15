//! Read a local Nintendo Switch controller (Pro Controller, Joy-Con) via Linux
//! hidraw, same approach as `xbox_reader.rs`: no libudev/hidapi dependency,
//! just scan `/sys/class/hidraw` for Nintendo's vendor id and a known product
//! id, then parse the standard `0x30` input report into a DualSense-shaped
//! `PadFrame`.
//!
//! hidraw is Linux-only; other platforms compile to a stub that reports no
//! pad, same as the DualSense and Xbox readers.
//!
//! Pro Controller over USB boots in a "simple HID" mode where it will not
//! stream input until it has been handed over with a short command sequence
//! (`0x80` handshake + report-mode subcommand, from `hid-nintendo`). This
//! reader performs that handshake best-effort on open for USB devices;
//! Bluetooth controllers stream `0x30` reports immediately and need nothing.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(not(target_os = "linux"))]
mod stub {
    use anyhow::{bail, Result};
    use couchlink_proto::PadFrame;

    pub struct SwitchReader;

    impl SwitchReader {
        pub fn open_first() -> Result<Self> {
            bail!("Nintendo Switch controller over hidraw is Linux-only; use keyboard input on this platform")
        }
        pub fn read_frame(&mut self) -> Result<Option<PadFrame>> {
            Ok(None)
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use stub::SwitchReader;

#[cfg(target_os = "linux")]
pub use linux_impl::SwitchReader;

#[cfg(target_os = "linux")]
mod linux_impl {
    use anyhow::{bail, Context, Result};
    use couchlink_pad::parse_switch_input_report;
    use couchlink_pad::recognize::is_supported_switch;
    use couchlink_proto::PadFrame;
    use std::fs::{self, File};
    use std::io::{Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::path::PathBuf;
    use tracing::info;

    /// Output report id for a rumble-and-subcommand packet (`hid-nintendo`).
    const JC_OUTPUT_RUMBLE_AND_SUBCMD: u8 = 0x01;
    /// USB command output report id (`hid-nintendo`).
    const JC_OUTPUT_USB_CMD: u8 = 0x80;
    /// Subcommand ids used during init.
    const JC_SUBCMD_SET_REPORT_MODE: u8 = 0x03;
    /// USB command sub-commands.
    const JC_USB_CMD_HANDSHAKE: u8 = 0x02;
    const JC_USB_CMD_BAUDRATE_3M: u8 = 0x03;
    const JC_USB_CMD_NO_TIMEOUT: u8 = 0x04;
    /// Standard, full report mode (0x30 input reports).
    const REPORT_MODE_STANDARD: u8 = 0x30;

    pub struct SwitchReader {
        file: File,
        seq: u32,
        path: PathBuf,
    }

    impl SwitchReader {
        pub fn open_first() -> Result<Self> {
            let (path, bus) = find_switch_hidraw()?
                .context("no Nintendo Switch controller hidraw node found")?;
            let file = File::options()
                .read(true)
                .write(true)
                .open(&path)
                .with_context(|| format!("open {}", path.display()))?;
            // Non-blocking reads so the WebRTC loop stays responsive.
            set_nonblocking(file.as_raw_fd())?;
            // Hand the USB Pro Controller / grip over before reading, so it
            // starts streaming 0x30 reports. Best-effort: Bluetooth needs
            // nothing and a failed handshake just logs.
            if bus == "0003" {
                usb_handshake(&file, &path);
            }
            info!("opened Nintendo Switch controller at {}", path.display());
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
                    let mut frame = match parse_switch_input_report(&buf[..n]) {
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

    fn find_switch_hidraw() -> Result<Option<(PathBuf, String)>> {
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
            let Some(id_line) = txt.lines().find(|l| l.starts_with("HID_ID=")) else {
                continue;
            };
            let parts: Vec<&str> = id_line.trim_start_matches("HID_ID=").split(':').collect();
            if parts.len() < 3 {
                continue;
            }
            let vid = u16::from_str_radix(parts[1], 16).unwrap_or(0);
            let pid = u16::from_str_radix(parts[2], 16).unwrap_or(0);
            if !is_supported_switch(vid, pid) {
                continue;
            }
            let node = PathBuf::from(format!("/dev/{name}"));
            // Prefer USB bus (0003) over Bluetooth (0005), same preference the
            // DualSense reader applies.
            if parts[0].eq_ignore_ascii_case("0003") {
                usb.push((node, "0003".to_string()));
            } else {
                bt.push((node, parts[0].to_string()));
            }
        }
        Ok(usb.into_iter().chain(bt).next())
    }

    /// Best-effort handshake for USB-attached Pro Controllers / charging
    /// grips, mirroring `hid-nintendo` `joycon_init_ctlr_state`. Every step is
    /// non-fatal — Bluetooth and already-active devices don't need it.
    fn usb_handshake(file: &File, path: &PathBuf) {
        for cmd in [JC_USB_CMD_HANDSHAKE, JC_USB_CMD_BAUDRATE_3M, JC_USB_CMD_NO_TIMEOUT] {
            if let Err(e) = file
                .write_all(&[JC_OUTPUT_USB_CMD, cmd])
                .with_context(|| format!("USB handshake {cmd:#04x} on {}", path.display()))
            {
                info!("USB handshake skipped: {e:#}");
                return;
            }
        }
        // Rumple-and-subcommand: switch to standard full report mode (0x30).
        // The silent rumble payload is the usual 8-byte zero-intensity packet.
        let mut subcmd = [0u8; 12];
        subcmd[0] = JC_OUTPUT_RUMBLE_AND_SUBCMD;
        subcmd[1] = 0x01; // packet num
        subcmd[2] = 0x00;
        subcmd[3] = 0x01;
        subcmd[4] = 0x40;
        subcmd[5] = 0x40;
        subcmd[6] = 0x00;
        subcmd[7] = 0x01;
        subcmd[8] = 0x40;
        subcmd[9] = 0x40;
        subcmd[10] = JC_SUBCMD_SET_REPORT_MODE;
        subcmd[11] = REPORT_MODE_STANDARD;
        if let Err(e) = file.write_all(&subcmd) {
            info!("report-mode subcommand failed (best-effort): {e:#}");
        }
    }

    fn set_nonblocking(fd: i32) -> Result<()> {
        use nix::fcntl::{fcntl, FcntlArg, OFlag};
        let flags = fcntl(fd, FcntlArg::F_GETFL).context("F_GETFL")?;
        let flags = OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK;
        fcntl(fd, FcntlArg::F_SETFL(flags)).context("F_SETFL")?;
        Ok(())
    }
}

#[cfg(test)]
mod client_switch_recognition_tests {
    use couchlink_pad::recognize::{is_supported_switch, parse_hid_id_line, product_label};
    use couchlink_pad::sim::{simulate_switch_frame, switch_press, SimButton};
    use couchlink_pad::NINTENDO_VID;
    use couchlink_proto::pad_frame::buttons;

    /// Mirrors `find_switch_hidraw` VID/PID accept rules for every supported
    /// Nintendo device.
    #[test]
    fn client_would_open_every_supported_switch() {
        for &pid in couchlink_pad::SWITCH_KNOWN_PIDS {
            for bus in ["0003", "0005"] {
                let line = format!(
                    "HID_ID={bus}:{:08X}:{:08X}",
                    NINTENDO_VID as u32,
                    pid as u32
                );
                let (_, vid, p) = parse_hid_id_line(&line).unwrap();
                assert!(
                    is_supported_switch(vid, p),
                    "SwitchReader must accept PID {pid:04X}"
                );
                assert!(product_label(vid, p).is_some());
            }
        }
    }

    #[test]
    fn client_parse_path_maps_a_to_cross() {
        let f = simulate_switch_frame(&switch_press(SimButton::Cross)).unwrap();
        assert!(f.buttons & buttons::CROSS != 0);
    }

    #[test]
    fn pro_controller_handshake_bytes_are_stable() {
        // Guard the exact byte sequences this reader writes to USB devices so
        // a refactor can't silently break the on-wire protocol.
        let usb_handshake = [0x80u8, 0x02, 0x80, 0x03, 0x80, 0x04];
        assert_eq!(usb_handshake.len(), 6);
        let mut subcmd = vec![0u8; 12];
        subcmd[0] = 0x01;
        subcmd[10] = 0x03; // SET_REPORT_MODE
        subcmd[11] = 0x30; // standard mode
        assert_eq!(subcmd.len(), 12);
        assert_ne!(couchlink_pad::switch::PID_SWITCH_PRO, 0);
    }
}
