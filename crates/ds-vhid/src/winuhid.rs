//! Optional WinUHid DualSense (`054c:0ce6`) backend via `WinUHidDevs.dll`.
//!
//! Install: https://github.com/cgutman/WinUHid — when the DLL is present we
//! prefer true DualSense over ViGEm DS4 so games can push adaptive triggers.

use std::ffi::c_void;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use couchlink_pad::feedback::build_usb_output_report;
use couchlink_pad::vhid_proto::DS_USB_INPUT_LEN;
use couchlink_proto::PadFeedback;
use tracing::{info, warn};
use windows::core::s;
use windows::Win32::Foundation::{BOOL, TRUE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

use crate::session::OutputHub;

type Ps5Gamepad = c_void;

#[repr(C)]
struct Ps5TriggerEffect {
    type_: u8,
    data: [u8; 10],
}

type FnCreate = unsafe extern "C" fn(
    info: *const c_void,
    rumble_cb: Option<unsafe extern "C" fn(*mut c_void, u8, u8)>,
    lightbar_cb: Option<unsafe extern "C" fn(*mut c_void, u8, u8, u8)>,
    player_led_cb: Option<unsafe extern "C" fn(*mut c_void, u8)>,
    trigger_cb: Option<
        unsafe extern "C" fn(*mut c_void, *const Ps5TriggerEffect, *const Ps5TriggerEffect),
    >,
    ctx: *mut c_void,
) -> *mut Ps5Gamepad;

type FnDestroy = unsafe extern "C" fn(*mut Ps5Gamepad);
type FnInitReport = unsafe extern "C" fn(*mut u8);
type FnReportInput = unsafe extern "C" fn(*mut Ps5Gamepad, *const u8) -> BOOL;

struct Api {
    // Intentionally leaked for process lifetime — not freed on Drop.
    create: FnCreate,
    destroy: FnDestroy,
    init_report: FnInitReport,
    report_input: FnReportInput,
}

fn try_load() -> Result<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    let slot = API.get_or_init(|| match load_api() {
        Ok(a) => Some(a),
        Err(e) => {
            warn!("WinUHidDevs.dll unavailable: {e:#}");
            None
        }
    });
    slot.as_ref()
        .ok_or_else(|| anyhow::anyhow!("WinUHidDevs.dll not loaded"))
}

fn load_api() -> Result<Api> {
    unsafe {
        let lib = LoadLibraryA(s!("WinUHidDevs.dll")).context("LoadLibrary WinUHidDevs.dll")?;
        let create: FnCreate = std::mem::transmute(
            GetProcAddress(lib, s!("WinUHidPS5Create")).context("WinUHidPS5Create")?,
        );
        let destroy: FnDestroy = std::mem::transmute(
            GetProcAddress(lib, s!("WinUHidPS5Destroy")).context("WinUHidPS5Destroy")?,
        );
        let init_report: FnInitReport = std::mem::transmute(
            GetProcAddress(lib, s!("WinUHidPS5InitializeInputReport"))
                .context("WinUHidPS5InitializeInputReport")?,
        );
        let report_input: FnReportInput = std::mem::transmute(
            GetProcAddress(lib, s!("WinUHidPS5ReportInput")).context("WinUHidPS5ReportInput")?,
        );
        // Keep DLL loaded for process lifetime (function pointers stay valid).
        std::mem::forget(lib);
        Ok(Api {
            create,
            destroy,
            init_report,
            report_input,
        })
    }
}

/// True when `WinUHidDevs.dll` can be loaded (driver MSI installed / on PATH).
pub fn available() -> bool {
    try_load().is_ok()
}

pub struct WinUhidDualSense {
    api: &'static Api,
    gamepad: *mut Ps5Gamepad,
    /// Kept alive for C callbacks (`CallbackContext`).
    _hub: Box<OutputHub>,
}

// WinUHid callbacks may fire from driver threads; OutputHub is Sync via Mutex.
unsafe impl Send for WinUhidDualSense {}

impl WinUhidDualSense {
    pub fn create(hub: OutputHub) -> Result<Self> {
        let api = try_load()?;
        let hub_box = Box::new(hub);
        let ctx = (&*hub_box as *const OutputHub) as *mut c_void;
        let gamepad = unsafe {
            (api.create)(
                std::ptr::null(),
                Some(on_rumble),
                Some(on_lightbar),
                Some(on_player_led),
                Some(on_triggers),
                ctx,
            )
        };
        if gamepad.is_null() {
            bail!("WinUHidPS5Create failed — is the WinUHid driver installed?");
        }
        info!("WinUHid DualSense plugged (P2, 054c:0ce6) — rumble/AT/lightbar → DSVO");
        Ok(Self {
            api,
            gamepad,
            _hub: hub_box,
        })
    }
}

impl crate::backend::PadBackend for WinUhidDualSense {
    fn apply_ds_report(&mut self, report: &[u8; DS_USB_INPUT_LEN]) -> Result<()> {
        // WINUHID_PS5_INPUT_REPORT is 64 bytes and matches DualSense USB layout.
        let mut buf = *report;
        if buf[0] == 0 {
            unsafe { (self.api.init_report)(buf.as_mut_ptr()) };
            buf = *report;
            buf[0] = 0x01;
        }
        let ok = unsafe { (self.api.report_input)(self.gamepad, buf.as_ptr()) };
        if ok != TRUE {
            bail!("WinUHidPS5ReportInput failed");
        }
        Ok(())
    }
}

impl Drop for WinUhidDualSense {
    fn drop(&mut self) {
        if !self.gamepad.is_null() {
            unsafe { (self.api.destroy)(self.gamepad) };
            self.gamepad = std::ptr::null_mut();
        }
    }
}

unsafe extern "C" fn on_rumble(ctx: *mut c_void, left: u8, right: u8) {
    if ctx.is_null() {
        return;
    }
    let hub = &*(ctx as *const OutputHub);
    let report = build_usb_output_report(&PadFeedback::Rumble {
        large: left,
        small: right,
    });
    hub.broadcast(report.to_vec());
}

unsafe extern "C" fn on_lightbar(ctx: *mut c_void, r: u8, g: u8, b: u8) {
    if ctx.is_null() {
        return;
    }
    let hub = &*(ctx as *const OutputHub);
    let report = build_usb_output_report(&PadFeedback::Lightbar { r, g, b });
    hub.broadcast(report.to_vec());
}

unsafe extern "C" fn on_player_led(ctx: *mut c_void, led: u8) {
    if ctx.is_null() {
        return;
    }
    let hub = &*(ctx as *const OutputHub);
    let report = build_usb_output_report(&PadFeedback::PlayerLed { mask: led });
    hub.broadcast(report.to_vec());
}

unsafe extern "C" fn on_triggers(
    ctx: *mut c_void,
    left: *const Ps5TriggerEffect,
    right: *const Ps5TriggerEffect,
) {
    if ctx.is_null() {
        return;
    }
    let hub = &*(ctx as *const OutputHub);
    let (left_mode, left_params) = if left.is_null() {
        (0u8, Vec::new())
    } else {
        let l = &*left;
        (l.type_, l.data.to_vec())
    };
    let (right_mode, right_params) = if right.is_null() {
        (0u8, Vec::new())
    } else {
        let r = &*right;
        (r.type_, r.data.to_vec())
    };
    let report = build_usb_output_report(&PadFeedback::AdaptiveTriggers {
        left_mode,
        left_params,
        right_mode,
        right_params,
    });
    hub.broadcast(report.to_vec());
}
