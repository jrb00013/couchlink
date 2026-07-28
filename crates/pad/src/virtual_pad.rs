//! Virtual DualSense presented as a **Bluetooth** gamepad on the host.
//!
//! Linux: `uinput` with `BUS_BLUETOOTH`, Sony VID/PID, and DualSense product name
//! so PCSX2 / RPCS3 enumerate it like a real wireless pad (same idea dualsensekit
//! uses when binding RPCS3 player slots to DualSense HID endpoints).

use anyhow::{bail, Context, Result};
use couchlink_proto::pad_frame::buttons;
use couchlink_proto::PadFrame;
use tracing::info;

use crate::dualsense::{PID_DUALSENSE, PRODUCT_NAME, SONY_VID};

#[derive(Debug, Clone)]
pub struct VirtualPadConfig {
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
    /// When true, set bus type to Bluetooth so udev/emulators treat it as wireless.
    pub as_bluetooth: bool,
}

impl Default for VirtualPadConfig {
    fn default() -> Self {
        Self {
            name: PRODUCT_NAME.into(),
            vendor: SONY_VID,
            product: PID_DUALSENSE,
            version: 0x0111,
            as_bluetooth: true,
        }
    }
}

pub struct VirtualPad {
    #[cfg(target_os = "linux")]
    inner: linux::LinuxUInput,
    #[cfg(not(target_os = "linux"))]
    _cfg: VirtualPadConfig,
}

impl VirtualPad {
    pub fn create(cfg: VirtualPadConfig) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let inner = linux::LinuxUInput::create(&cfg)?;
            info!(
                "virtual pad ready: '{}' vid={:04x} pid={:04x} bus={}",
                cfg.name,
                cfg.vendor,
                cfg.product,
                if cfg.as_bluetooth {
                    "bluetooth"
                } else {
                    "usb"
                }
            );
            Ok(Self { inner })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = cfg;
            bail!("virtual Bluetooth pad injection is currently implemented for Linux uinput; Windows ViGEm path planned")
        }
    }

    pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.inner.apply(frame)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = frame;
            Ok(())
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    // linux/input-event-codes.h / uinput.h subset
    const BUS_USB: u16 = 0x03;
    const BUS_BLUETOOTH: u16 = 0x05;
    const EV_SYN: u16 = 0x00;
    const EV_KEY: u16 = 0x01;
    const EV_ABS: u16 = 0x03;
    const SYN_REPORT: u16 = 0;
    const UI_SET_EVBIT: u64 = 0x40045564;
    const UI_SET_KEYBIT: u64 = 0x40045565;
    const UI_SET_ABSBIT: u64 = 0x40045567;
    const UI_DEV_SETUP: u64 = 0x405c5503;
    const UI_DEV_CREATE: u64 = 0x00005501;
    const UI_DEV_DESTROY: u64 = 0x00005502;

    // Buttons
    const BTN_SOUTH: u16 = 0x130;
    const BTN_EAST: u16 = 0x131;
    const BTN_NORTH: u16 = 0x133;
    const BTN_WEST: u16 = 0x134;
    const BTN_TL: u16 = 0x136;
    const BTN_TR: u16 = 0x137;
    const BTN_TL2: u16 = 0x138;
    const BTN_TR2: u16 = 0x139;
    const BTN_SELECT: u16 = 0x13a;
    const BTN_START: u16 = 0x13b;
    const BTN_MODE: u16 = 0x13c;
    const BTN_THUMBL: u16 = 0x13d;
    const BTN_THUMBR: u16 = 0x13e;
    const BTN_DPAD_UP: u16 = 0x220;
    const BTN_DPAD_DOWN: u16 = 0x221;
    const BTN_DPAD_LEFT: u16 = 0x222;
    const BTN_DPAD_RIGHT: u16 = 0x223;

    const ABS_X: u16 = 0x00;
    const ABS_Y: u16 = 0x01;
    const ABS_RX: u16 = 0x03;
    const ABS_RY: u16 = 0x04;
    const ABS_Z: u16 = 0x02; // L2
    const ABS_RZ: u16 = 0x05; // R2

    #[repr(C)]
    struct InputId {
        bustype: u16,
        vendor: u16,
        product: u16,
        version: u16,
    }

    #[repr(C)]
    struct UinputSetup {
        id: InputId,
        name: [u8; 80],
        ff_effects_max: u32,
    }

    #[repr(C)]
    struct InputEvent {
        time_sec: usize,
        time_usec: usize,
        type_: u16,
        code: u16,
        value: i32,
    }

    pub struct LinuxUInput {
        file: std::fs::File,
    }

    impl LinuxUInput {
        pub fn create(cfg: &VirtualPadConfig) -> Result<Self> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc_nonblock())
                .open("/dev/uinput")
                .context("open /dev/uinput (need uinput module + permissions)")?;

            unsafe {
                ioctl_set(file.as_raw_fd(), UI_SET_EVBIT, EV_KEY as u64)?;
                ioctl_set(file.as_raw_fd(), UI_SET_EVBIT, EV_ABS as u64)?;
                ioctl_set(file.as_raw_fd(), UI_SET_EVBIT, EV_SYN as u64)?;
                for code in [
                    BTN_SOUTH, BTN_EAST, BTN_NORTH, BTN_WEST, BTN_TL, BTN_TR, BTN_TL2, BTN_TR2,
                    BTN_SELECT, BTN_START, BTN_MODE, BTN_THUMBL, BTN_THUMBR, BTN_DPAD_UP,
                    BTN_DPAD_DOWN, BTN_DPAD_LEFT, BTN_DPAD_RIGHT,
                ] {
                    ioctl_set(file.as_raw_fd(), UI_SET_KEYBIT, code as u64)?;
                }
                for code in [ABS_X, ABS_Y, ABS_RX, ABS_RY, ABS_Z, ABS_RZ] {
                    ioctl_set(file.as_raw_fd(), UI_SET_ABSBIT, code as u64)?;
                }
            }

            let mut setup: UinputSetup = unsafe { std::mem::zeroed() };
            setup.id.bustype = if cfg.as_bluetooth {
                BUS_BLUETOOTH
            } else {
                BUS_USB
            };
            setup.id.vendor = cfg.vendor;
            setup.id.product = cfg.product;
            setup.id.version = cfg.version;
            let name_bytes = cfg.name.as_bytes();
            let n = name_bytes.len().min(79);
            setup.name[..n].copy_from_slice(&name_bytes[..n]);

            unsafe {
                let ret = libc_ioctl(
                    file.as_raw_fd(),
                    UI_DEV_SETUP,
                    &setup as *const _ as u64,
                );
                if ret < 0 {
                    bail!("UI_DEV_SETUP failed");
                }
                let ret = libc_ioctl(file.as_raw_fd(), UI_DEV_CREATE, 0);
                if ret < 0 {
                    bail!("UI_DEV_CREATE failed");
                }
            }

            // Give udev a moment to create /dev/input/event*
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(Self { file })
        }

        pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
            let b = frame.buttons;
            self.emit_key(BTN_WEST, b & buttons::SQUARE != 0)?;
            self.emit_key(BTN_SOUTH, b & buttons::CROSS != 0)?;
            self.emit_key(BTN_EAST, b & buttons::CIRCLE != 0)?;
            self.emit_key(BTN_NORTH, b & buttons::TRIANGLE != 0)?;
            self.emit_key(BTN_TL, b & buttons::L1 != 0)?;
            self.emit_key(BTN_TR, b & buttons::R1 != 0)?;
            self.emit_key(BTN_TL2, b & buttons::L2 != 0)?;
            self.emit_key(BTN_TR2, b & buttons::R2 != 0)?;
            self.emit_key(BTN_SELECT, b & buttons::CREATE != 0)?;
            self.emit_key(BTN_START, b & buttons::OPTIONS != 0)?;
            self.emit_key(BTN_THUMBL, b & buttons::L3 != 0)?;
            self.emit_key(BTN_THUMBR, b & buttons::R3 != 0)?;
            self.emit_key(BTN_MODE, b & buttons::PS != 0)?;
            self.emit_key(BTN_DPAD_UP, b & buttons::DPAD_UP != 0)?;
            self.emit_key(BTN_DPAD_DOWN, b & buttons::DPAD_DOWN != 0)?;
            self.emit_key(BTN_DPAD_LEFT, b & buttons::DPAD_LEFT != 0)?;
            self.emit_key(BTN_DPAD_RIGHT, b & buttons::DPAD_RIGHT != 0)?;

            self.emit_abs(ABS_X, frame.lx as i32)?;
            self.emit_abs(ABS_Y, frame.ly as i32)?;
            self.emit_abs(ABS_RX, frame.rx as i32)?;
            self.emit_abs(ABS_RY, frame.ry as i32)?;
            self.emit_abs(ABS_Z, frame.l2 as i32)?;
            self.emit_abs(ABS_RZ, frame.r2 as i32)?;
            self.emit(EV_SYN, SYN_REPORT, 0)?;
            Ok(())
        }

        fn emit_key(&mut self, code: u16, down: bool) -> Result<()> {
            self.emit(EV_KEY, code, if down { 1 } else { 0 })
        }

        fn emit_abs(&mut self, code: u16, value: i32) -> Result<()> {
            self.emit(EV_ABS, code, value)
        }

        fn emit(&mut self, type_: u16, code: u16, value: i32) -> Result<()> {
            let ev = InputEvent {
                time_sec: 0,
                time_usec: 0,
                type_,
                code,
                value,
            };
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    &ev as *const _ as *const u8,
                    std::mem::size_of::<InputEvent>(),
                )
            };
            self.file.write_all(bytes)?;
            Ok(())
        }
    }

    impl Drop for LinuxUInput {
        fn drop(&mut self) {
            unsafe {
                let _ = libc_ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY, 0);
            }
        }
    }

    fn libc_nonblock() -> i32 {
        0 // blocking is fine for injection
    }

    unsafe fn ioctl_set(fd: i32, req: u64, val: u64) -> Result<()> {
        let ret = libc_ioctl(fd, req, val);
        if ret < 0 {
            bail!("ioctl 0x{req:x} failed");
        }
        Ok(())
    }

    // Minimal ioctl without pulling full libc crate conflict — use nix
    unsafe fn libc_ioctl(fd: i32, req: u64, arg: u64) -> i32 {
        nix::libc::ioctl(fd, req as _, arg)
    }
}
