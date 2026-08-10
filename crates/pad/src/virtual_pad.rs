//! Virtual DualSense presented as a **Bluetooth** gamepad on the host.
//!
//! - **Linux / WSL:** Prefer DualSense VHID companion over TCP (Windows emulators),
//!   else local `/dev/uhid` DualSense, else `uinput` DualSense.
//! - **Windows:** DualSense VHID companion (pipe/TCP), else ViGEm DualShock 4 / Xbox 360.

use anyhow::Result;
#[cfg(any(target_os = "linux", all(not(target_os = "linux"), not(windows))))]
use anyhow::bail;
#[cfg(target_os = "linux")]
use anyhow::Context;
use couchlink_proto::{PadFeedback, PadFrame};
#[cfg(target_os = "linux")]
use couchlink_proto::pad_frame::buttons;
use tracing::info;

use crate::dualsense::{PID_DUALSENSE, PRODUCT_NAME, SONY_VID};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualPadBackend {
    /// Prefer DualSense VHID companion, then platform fallbacks (uinput / ViGEm).
    Auto,
    DualSense,
    Ds4,
    Xbox360,
    Noop,
}

impl VirtualPadBackend {
    pub fn from_env() -> Self {
        match std::env::var("COUCHLINK_VIRTUAL_PAD")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "dualsense" | "ds" | "ds5" => Self::DualSense,
            "ds4" | "dualshock4" | "ps4" => Self::Ds4,
            "xbox" | "xbox360" | "x360" => Self::Xbox360,
            "noop" | "none" | "off" => Self::Noop,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VirtualPadConfig {
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
    /// When true, set bus type to Bluetooth so udev/emulators treat it as wireless.
    pub as_bluetooth: bool,
    pub backend: VirtualPadBackend,
}

impl Default for VirtualPadConfig {
    fn default() -> Self {
        Self {
            name: PRODUCT_NAME.into(),
            vendor: SONY_VID,
            product: PID_DUALSENSE,
            version: 0x0111,
            as_bluetooth: true,
            backend: VirtualPadBackend::from_env(),
        }
    }
}

pub struct VirtualPad {
    #[cfg(target_os = "linux")]
    inner: linux::Inner,
    #[cfg(windows)]
    inner: windows_inner::Inner,
    #[cfg(all(not(target_os = "linux"), not(windows)))]
    _cfg: VirtualPadConfig,
}

impl VirtualPad {
    pub fn create(cfg: VirtualPadConfig) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            if matches!(cfg.backend, VirtualPadBackend::Noop) {
                return Ok(Self::create_noop(cfg));
            }
            let inner = linux::Inner::create(&cfg)?;
            Ok(Self { inner })
        }
        #[cfg(windows)]
        {
            let inner = windows_inner::Inner::create(&cfg)?;
            Ok(Self { inner })
        }
        #[cfg(all(not(target_os = "linux"), not(windows)))]
        {
            let _ = cfg;
            bail!("virtual pad injection is Linux (uinput/VHID) or Windows (VHID/ViGEm) only")
        }
    }

    /// Accept pad frames but do not inject. Used for video-only hosts and for
    /// controller tests that exercise the apply path without `/dev/uinput`.
    pub fn create_noop(cfg: VirtualPadConfig) -> Self {
        info!("virtual pad noop — no injection ('{}')", cfg.name);
        #[cfg(target_os = "linux")]
        {
            Self {
                inner: linux::Inner::Noop,
            }
        }
        #[cfg(windows)]
        {
            let _ = cfg;
            Self {
                inner: windows_inner::Inner::Noop,
            }
        }
        #[cfg(all(not(target_os = "linux"), not(windows)))]
        {
            Self { _cfg: cfg }
        }
    }

    pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            self.inner.apply(frame)
        }
        #[cfg(windows)]
        {
            self.inner.apply(frame)
        }
        #[cfg(all(not(target_os = "linux"), not(windows)))]
        {
            let _ = frame;
            Ok(())
        }
    }

    /// Drain game HID output forwarded by the DualSense VHID companion (if any).
    pub fn poll_feedback(&mut self) -> Result<Vec<PadFeedback>> {
        #[cfg(target_os = "linux")]
        {
            self.inner.poll_feedback()
        }
        #[cfg(windows)]
        {
            self.inner.poll_feedback()
        }
        #[cfg(all(not(target_os = "linux"), not(windows)))]
        {
            Ok(Vec::new())
        }
    }
}

#[cfg(windows)]
mod windows_inner {
    use super::*;
    use crate::windows_pad::WindowsPad;

    pub enum Inner {
        Live(WindowsPad),
        Noop,
    }

    impl Inner {
        pub fn create(cfg: &VirtualPadConfig) -> Result<Self> {
            if matches!(cfg.backend, VirtualPadBackend::Noop) {
                return Ok(Self::Noop);
            }
            Ok(Self::Live(WindowsPad::create(cfg)?))
        }

        pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
            match self {
                Self::Live(p) => p.apply(frame),
                Self::Noop => Ok(()),
            }
        }

        pub fn poll_feedback(&mut self) -> Result<Vec<PadFeedback>> {
            match self {
                Self::Live(p) => p.poll_feedback(),
                Self::Noop => Ok(Vec::new()),
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::vhid_client::{likely_wsl, VhidClient};
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use tracing::warn;

    pub enum Inner {
        Vhid(VhidClient),
        Uhid(crate::linux_uhid::LinuxUhid),
        UInput(LinuxUInput),
        Noop,
    }

    impl Inner {
        pub fn create(cfg: &VirtualPadConfig) -> Result<Self> {
            let env_vhid = std::env::var("COUCHLINK_DS_VHID")
                .map(|v| {
                    let v = v.to_ascii_lowercase();
                    v == "1" || v == "tcp" || v == "force" || v == "auto"
                })
                .unwrap_or(false);
            let try_vhid = matches!(cfg.backend, VirtualPadBackend::DualSense)
                || env_vhid
                || (matches!(cfg.backend, VirtualPadBackend::Auto) && likely_wsl());

            if try_vhid {
                match VhidClient::connect() {
                    Ok(c) => {
                        info!("Linux/WSL virtual pad: DualSense VHID companion (TCP/pipe)");
                        return Ok(Self::Vhid(c));
                    }
                    Err(e) if matches!(cfg.backend, VirtualPadBackend::DualSense) && likely_wsl() => {
                        return Err(e).context("DualSense VHID companion required under WSL");
                    }
                    Err(e) => {
                        warn!("VHID companion unavailable ({e:#}) — trying local backends");
                    }
                }
            }

            if matches!(cfg.backend, VirtualPadBackend::Ds4 | VirtualPadBackend::Xbox360) {
                warn!(
                    "backend {:?} is Windows-oriented — using Linux DualSense backends",
                    cfg.backend
                );
            }

            // Prefer /dev/uhid so hid-playstation can forward OUTPUT → friend.
            let try_uhid = matches!(
                cfg.backend,
                VirtualPadBackend::Auto | VirtualPadBackend::DualSense
            ) && crate::linux_uhid::uhid_available();
            if try_uhid {
                let id = crate::linux_uhid::UhidIdentity {
                    name: &cfg.name,
                    vendor: cfg.vendor,
                    product: cfg.product,
                    version: cfg.version,
                    as_bluetooth: cfg.as_bluetooth,
                };
                match crate::linux_uhid::LinuxUhid::create(&id) {
                    Ok(p) => return Ok(Self::Uhid(p)),
                    Err(e) if matches!(cfg.backend, VirtualPadBackend::DualSense) && !likely_wsl() => {
                        warn!("UHID DualSense failed ({e:#}) — falling back to uinput");
                    }
                    Err(e) => {
                        warn!("UHID DualSense unavailable ({e:#}) — falling back to uinput");
                    }
                }
            }

            let pad = LinuxUInput::create(cfg)?;
            info!(
                "virtual pad ready: '{}' vid={:04x} pid={:04x} bus={}",
                cfg.name,
                cfg.vendor,
                cfg.product,
                if cfg.as_bluetooth { "bluetooth" } else { "usb" }
            );
            Ok(Self::UInput(pad))
        }

        pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
            match self {
                Self::Vhid(c) => c.apply(frame),
                Self::Uhid(p) => p.apply(frame),
                Self::UInput(p) => p.apply(frame),
                Self::Noop => Ok(()),
            }
        }

        pub fn poll_feedback(&mut self) -> Result<Vec<PadFeedback>> {
            match self {
                Self::Vhid(c) => c.poll_feedback(),
                Self::Uhid(p) => p.poll_feedback(),
                Self::UInput(_) | Self::Noop => Ok(Vec::new()),
            }
        }
    }

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
    const ABS_Z: u16 = 0x02;
    const ABS_RZ: u16 = 0x05;

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
        file: Option<std::fs::File>,
    }

    impl LinuxUInput {
        pub fn create(cfg: &VirtualPadConfig) -> Result<Self> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(0)
                .open("/dev/uinput")
                .context("open /dev/uinput (need uinput module + permissions)")?;

            unsafe {
                ioctl_set(file.as_raw_fd(), UI_SET_EVBIT, EV_KEY as u64)?;
                ioctl_set(file.as_raw_fd(), UI_SET_EVBIT, EV_ABS as u64)?;
                ioctl_set(file.as_raw_fd(), UI_SET_EVBIT, EV_SYN as u64)?;
                for code in [
                    BTN_SOUTH,
                    BTN_EAST,
                    BTN_NORTH,
                    BTN_WEST,
                    BTN_TL,
                    BTN_TR,
                    BTN_TL2,
                    BTN_TR2,
                    BTN_SELECT,
                    BTN_START,
                    BTN_MODE,
                    BTN_THUMBL,
                    BTN_THUMBR,
                    BTN_DPAD_UP,
                    BTN_DPAD_DOWN,
                    BTN_DPAD_LEFT,
                    BTN_DPAD_RIGHT,
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
                let ret = libc_ioctl(file.as_raw_fd(), UI_DEV_SETUP, &setup as *const _ as u64);
                if ret < 0 {
                    bail!("UI_DEV_SETUP failed");
                }
                let ret = libc_ioctl(file.as_raw_fd(), UI_DEV_CREATE, 0);
                if ret < 0 {
                    bail!("UI_DEV_CREATE failed");
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(Self { file: Some(file) })
        }

        pub fn apply(&mut self, frame: &PadFrame) -> Result<()> {
            if self.file.is_none() {
                return Ok(());
            }
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
            let Some(file) = self.file.as_mut() else {
                return Ok(());
            };
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
            file.write_all(bytes)?;
            Ok(())
        }
    }

    impl Drop for LinuxUInput {
        fn drop(&mut self) {
            let Some(file) = self.file.as_ref() else {
                return;
            };
            unsafe {
                let _ = libc_ioctl(file.as_raw_fd(), UI_DEV_DESTROY, 0);
            }
        }
    }

    unsafe fn ioctl_set(fd: i32, req: u64, val: u64) -> Result<()> {
        let ret = libc_ioctl(fd, req, val);
        if ret < 0 {
            bail!("ioctl 0x{req:x} failed");
        }
        Ok(())
    }

    unsafe fn libc_ioctl(fd: i32, req: u64, arg: u64) -> i32 {
        nix::libc::ioctl(fd, req as _, arg)
    }
}
