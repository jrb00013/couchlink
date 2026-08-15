//! Read a local Steam Controller (classic V1) via Linux hidraw, same approach
//! as `xbox_reader.rs`: scan `/sys/class/hidraw` for Valve's vendor id and a
//! known product id, then parse the `0x01` controller-state report into a
//! DualSense-shaped `PadFrame`.
//!
//! hidraw is Linux-only; other platforms compile to a stub that reports no
//! pad, same as the DualSense and Xbox readers.
//!
//! Lizard mode: the controller boots emulating a mouse/keyboard and will not
//! report gamepad state until it is switched to gamepad mode. `hid-steam`
//! strips lizard mode when its input device is opened; opening the hidraw
//! node this reader uses also arms the Steam client interface, and we go one
//! step further and best-effort write the same feature reports `hid-steam`
//! / `sc-controller` use to disable the mouse/keyboard mappings and pad
//! modes. Without Steam or a `hid-steam` gamepad-mode switch the user may
//! still need to put the controller into gamepad mode first.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(not(target_os = "linux"))]
mod stub {
    use anyhow::{bail, Result};
    use couchlink_proto::PadFrame;

    pub struct SteamReader;

    impl SteamReader {
        pub fn open_first() -> Result<Self> {
            bail!("Steam Controller over hidraw is Linux-only; use keyboard input on this platform")
        }
        pub fn read_frame(&mut self) -> Result<Option<PadFrame>> {
            Ok(None)
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use stub::SteamReader;

#[cfg(target_os = "linux")]
pub use linux_impl::SteamReader;

#[cfg(target_os = "linux")]
mod linux_impl {
    use anyhow::{bail, Context, Result};
    use couchlink_pad::recognize::is_supported_steam_controller;
    use couchlink_pad::steam_controller::{
        ID_CLEAR_DIGITAL_MAPPINGS, ID_SET_SETTINGS_VALUES, SETTING_LEFT_TRACKPAD_MODE,
        SETTING_RIGHT_TRACKPAD_MODE, TRACKPAD_MODE_NONE,
    };
    use couchlink_pad::parse_steam_input_report;
    use couchlink_proto::PadFrame;
    use std::fs::{self, File};
    use std::io::Read;
    use std::os::unix::io::AsRawFd;
    use std::path::PathBuf;
    use tracing::info;

    /// Feature reports on the Steam Controller use report id 0; the command
    /// byte follows it (see `hid-steam` `steam_send_report`).
    const FEATURE_REPORT_SIZE: usize = 64;

    /// `HIDIOCSFEATURE(len)` = `_IOWR('H', 0x06, len)`.
    fn hidio_sfeature(len: usize) -> usize {
        (3u64 << 30) | ((len as u64) << 16) | (0x48 << 8) | 0x06
    }

    pub struct SteamReader {
        file: File,
        seq: u32,
        path: PathBuf,
    }

    impl SteamReader {
        pub fn open_first() -> Result<Self> {
            let path = find_steam_hidraw()?
                .context("no Steam Controller hidraw node found")?;
            let file = File::options()
                .read(true)
                .write(true)
                .open(&path)
                .with_context(|| format!("open {}", path.display()))?;
            // Non-blocking reads so the WebRTC loop stays responsive.
            set_nonblocking(file.as_raw_fd())?;
            // Best-effort gamepad-mode switch; never fatal.
            switch_to_gamepad_mode(file.as_raw_fd(), &path);
            info!("opened Steam Controller at {}", path.display());
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
                    let mut frame = match parse_steam_input_report(&buf[..n]) {
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

    fn find_steam_hidraw() -> Result<Option<PathBuf>> {
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
            if !is_supported_steam_controller(vid, pid) {
                continue;
            }
            let node = PathBuf::from(format!("/dev/{name}"));
            if parts[0].eq_ignore_ascii_case("0003") {
                usb.push(node);
            } else {
                bt.push(node);
            }
        }
        Ok(usb.into_iter().chain(bt).next())
    }

    /// Best-effort `steam_set_lizard_mode(false)`: clear the mouse/keyboard
    /// digital mappings and disable both trackpads' mouse mode via a settings
    /// feature report. Any failure just logs; the reader still works whenever
    /// a gamepad-mode switch already happened (Steam, sc-controller, hid-steam).
    fn switch_to_gamepad_mode(fd: i32, path: &PathBuf) {
        let mut buf = [0u8; FEATURE_REPORT_SIZE];
        // ID_CLEAR_DIGITAL_MAPPINGS (0x81): drop esc/enter/cursor emulation.
        buf[1] = ID_CLEAR_DIGITAL_MAPPINGS;
        send_feature(fd, &mut buf, path);

        // ID_SET_SETTINGS_VALUES (0x87): set both trackpad modes to "none".
        let mut buf = [0u8; FEATURE_REPORT_SIZE];
        buf[1] = ID_SET_SETTINGS_VALUES;
        buf[2] = 0x06; // 2 settings × (reg + u16 value)
        buf[3] = SETTING_LEFT_TRACKPAD_MODE;
        buf[4] = TRACKPAD_MODE_NONE as u8;
        buf[5] = (TRACKPAD_MODE_NONE >> 8) as u8;
        buf[6] = SETTING_RIGHT_TRACKPAD_MODE;
        buf[7] = TRACKPAD_MODE_NONE as u8;
        buf[8] = (TRACKPAD_MODE_NONE >> 8) as u8;
        send_feature(fd, &mut buf, path);
    }

    fn send_feature(fd: i32, buf: &mut [u8], path: &PathBuf) {
        let request = hidio_sfeature(buf.len());
        let rc = unsafe { nix::libc::ioctl(fd, request as _, buf.as_mut_ptr()) };
        if rc < 0 {
            let e = std::io::Error::last_os_error();
            info!("Steam feature report on {} best-effort failed: {e}", path.display());
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
mod client_steam_recognition_tests {
    use couchlink_pad::recognize::{is_supported_steam_controller, parse_hid_id_line, product_label};
    use couchlink_pad::sim::{simulate_steam_frame, steam_press, SimButton};
    use couchlink_pad::VALVE_VID;
    use couchlink_proto::pad_frame::buttons;

    /// Mirrors `find_steam_hidraw` VID/PID accept rules for every Steam
    /// Controller presentation (dongle / bluetooth / wired).
    #[test]
    fn client_would_open_every_supported_steam_controller() {
        for &pid in couchlink_pad::STEAM_KNOWN_PIDS {
            for bus in ["0003", "0005"] {
                let line = format!(
                    "HID_ID={bus}:{:08X}:{:08X}",
                    VALVE_VID as u32,
                    pid as u32
                );
                let (_, vid, p) = parse_hid_id_line(&line).unwrap();
                assert!(
                    is_supported_steam_controller(vid, p),
                    "SteamReader must accept PID {pid:04X}"
                );
                assert!(product_label(vid, p).is_some());
            }
        }
    }

    #[test]
    fn client_parse_path_maps_a_to_cross() {
        let f = simulate_steam_frame(&steam_press(SimButton::Cross)).unwrap();
        assert!(f.buttons & buttons::CROSS != 0);
    }
}
