//! Keep a captured window rendering even when the user "minimizes" it.
//!
//! DWM stops compositing a minimized window — it is resized to nothing and issues no
//! draw calls — so Windows Graphics Capture has no texture to hand us and the stream
//! freezes. The fix used by streaming and remote-desktop tools is to never let the
//! window actually minimize: restore it and park it far off-screen instead. The OS
//! still considers it open and keeps rendering it at full size, while the user sees
//! it disappear.
//!
//! Unlike the subclassing/`SetWindowsHookEx` approach, this needs no code injected
//! into the target process: `SetWindowPos` and `ShowWindow` work cross-process on any
//! HWND. We poll for the minimized state instead of intercepting `WM_SYSCOMMAND`.
//!
//! We are moving a window belonging to an app we did not write, so the original
//! placement is always saved first and restored when capture stops, when the user
//! brings the window back to the foreground, or on Ctrl-C.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tracing::{info, warn};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowPlacement, IsIconic, IsWindow, SetWindowPlacement, SetWindowPos,
    ShowWindow, SW_RESTORE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, WINDOWPLACEMENT,
};

/// Far enough off-screen that no monitor arrangement can show it.
const PARK_X: i32 = -32000;
const PARK_Y: i32 = -32000;
const POLL: Duration = Duration::from_millis(200);

/// Saved placement of the window we parked, so any exit path can put it back.
struct Parked {
    hwnd: isize,
    placement: WINDOWPLACEMENT,
}

// SAFETY: only the raw handle value and a plain-old-data struct cross threads; every
// Win32 call is made from whichever thread holds the lock, which is legal for these
// APIs (they post to the owning thread's message queue).
unsafe impl Send for Parked {}

static PARKED: OnceLock<Mutex<Option<Parked>>> = OnceLock::new();
static STOP: AtomicBool = AtomicBool::new(false);

fn parked() -> &'static Mutex<Option<Parked>> {
    PARKED.get_or_init(|| Mutex::new(None))
}

fn empty_placement() -> WINDOWPLACEMENT {
    WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        ..Default::default()
    }
}

/// Put a parked window back exactly where the user had it. Safe to call repeatedly.
pub fn restore_now() {
    let Ok(mut guard) = parked().lock() else {
        return;
    };
    let Some(p) = guard.take() else {
        return;
    };
    let hwnd = HWND(p.hwnd as *mut std::ffi::c_void);
    unsafe {
        if IsWindow(Some(hwnd)).as_bool() {
            if let Err(e) = SetWindowPlacement(hwnd, &p.placement) {
                warn!("could not restore window placement: {e}");
            } else {
                info!("restored the captured window to its original position");
            }
        }
    }
}

unsafe extern "system" fn ctrl_handler(_ctrl_type: u32) -> windows::core::BOOL {
    restore_now();
    // Returning FALSE lets the default handler run and terminate us as usual.
    windows::core::BOOL(0)
}

/// Watch `hwnd` and keep it composited. Returns immediately; the watcher runs until
/// [`stop`] is called or the window goes away.
pub fn spawn(hwnd_raw: *mut std::ffi::c_void) {
    let hwnd_val = hwnd_raw as isize;
    unsafe {
        // Restore the window even if the user Ctrl-Cs the capture process.
        if let Err(e) = SetConsoleCtrlHandler(Some(ctrl_handler), true) {
            warn!("no Ctrl-C handler ({e}) — window may stay off-screen if killed");
        }
    }

    std::thread::spawn(move || {
        let hwnd = HWND(hwnd_val as *mut std::ffi::c_void);
        info!("keep-rendering active: minimizing this window will park it off-screen instead");
        while !STOP.load(Ordering::Relaxed) {
            std::thread::sleep(POLL);
            unsafe {
                if !IsWindow(Some(hwnd)).as_bool() {
                    info!("captured window closed — keep-rendering watcher stopping");
                    return;
                }
                let is_parked = parked().lock().map(|g| g.is_some()).unwrap_or(false);

                if !is_parked && IsIconic(hwnd).as_bool() {
                    park(hwnd, hwnd_val);
                } else if is_parked && GetForegroundWindow() == hwnd {
                    // The user brought it back (taskbar click): give it its spot back.
                    restore_now();
                }
            }
        }
        restore_now();
    });
}

/// Stop watching and put the window back.
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
    restore_now();
}

unsafe fn park(hwnd: HWND, hwnd_val: isize) {
    let mut placement = empty_placement();
    if let Err(e) = GetWindowPlacement(hwnd, &mut placement) {
        warn!("could not read window placement ({e}) — leaving it minimized");
        return;
    }
    // Un-minimize so DWM resumes compositing, then hide it off-screen. Size is left
    // untouched (SWP_NOSIZE) so the capture keeps its resolution.
    let _ = ShowWindow(hwnd, SW_RESTORE);
    if let Err(e) = SetWindowPos(
        hwnd,
        None,
        PARK_X,
        PARK_Y,
        0,
        0,
        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
    ) {
        warn!("could not move window off-screen ({e}) — restoring it");
        let _ = SetWindowPlacement(hwnd, &placement);
        return;
    }
    if let Ok(mut guard) = parked().lock() {
        *guard = Some(Parked {
            hwnd: hwnd_val,
            placement,
        });
    }
    info!("window minimized — parked off-screen to keep frames flowing");
}
