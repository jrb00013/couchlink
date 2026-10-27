//! Keep viewer pad input out of whatever app the host is using.
//!
//! The virtual pad is a real controller to Windows, delivered to the foreground window. Windows
//! Terminal (WinUI) treats D-pad/A/B as focus navigation, so a friend playing while the host sat
//! in a terminal moved the highlight along the tab strip and closed every tab. This gate swaps
//! the report for a neutral one while such a window is in front, so the pad is only "live" for
//! the game (and every other app that does not act on controller input).
//!
//! - `COUCHLINK_INPUT_BLOCK_FOREGROUND` - comma list of exe names that must never receive pad
//!   input. Default: terminals and shells. `off` disables the gate.
//! - `COUCHLINK_INPUT_ONLY_FOREGROUND` - if set (e.g. `rpcs3.exe`), pad input is delivered ONLY
//!   while one of these is in front; takes priority over the block list. Stricter, but friends
//!   stop playing whenever the host alt-tabs away.

const DEFAULT_BLOCK: &[&str] = &[
    "windowsterminal.exe", "wt.exe", "openconsole.exe", "conhost.exe", "cmd.exe",
    "powershell.exe", "pwsh.exe", "wsl.exe", "wslhost.exe", "explorer.exe",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Policy {
    Off,
    Block(Vec<String>),
    Only(Vec<String>),
}

fn list(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty()).collect()
}

pub fn policy_from_env() -> Policy {
    if let Ok(only) = std::env::var("COUCHLINK_INPUT_ONLY_FOREGROUND") {
        let l = list(&only);
        if !l.is_empty() {
            return Policy::Only(l);
        }
    }
    match std::env::var("COUCHLINK_INPUT_BLOCK_FOREGROUND") {
        Ok(v) if v.trim().eq_ignore_ascii_case("off") => Policy::Off,
        Ok(v) if !list(&v).is_empty() => Policy::Block(list(&v)),
        _ => Policy::Block(DEFAULT_BLOCK.iter().map(|s| s.to_string()).collect()),
    }
}

/// Should pad input reach the host given the foreground exe (lowercase file name, if known)?
pub fn allows(policy: &Policy, foreground_exe: Option<&str>) -> bool {
    match (policy, foreground_exe) {
        (Policy::Off, _) => true,
        (Policy::Block(_), None) => true,
        (Policy::Block(l), Some(e)) => !l.iter().any(|x| x == e),
        (Policy::Only(l), Some(e)) => l.iter().any(|x| x == e),
        (Policy::Only(_), None) => false,
    }
}

#[cfg(windows)]
fn foreground_exe() -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit(['\\', '/']).next().map(|s| s.to_ascii_lowercase())
    }
}

#[cfg(not(windows))]
fn foreground_exe() -> Option<String> {
    None
}

/// How often the foreground window is sampled (off the report path).
const POLL: std::time::Duration = std::time::Duration::from_millis(50);

static ALLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether pad input may reach the host right now. One relaxed atomic load on the report path;
/// a background thread (started on first call) samples the foreground window every [`POLL`].
pub fn input_allowed() -> bool {
    use std::sync::atomic::Ordering;
    static POLLER: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    POLLER.get_or_init(|| {
        let policy = policy_from_env();
        tracing::info!("input focus gate: {policy:?}");
        if policy == Policy::Off {
            return;
        }
        let _ = std::thread::Builder::new().name("focus-gate".into()).spawn(move || loop {
            let exe = foreground_exe();
            let ok = allows(&policy, exe.as_deref());
            if ALLOWED.swap(ok, Ordering::Relaxed) != ok {
                tracing::info!(
                    "viewer input {} (foreground = {})",
                    if ok { "live again" } else { "BLOCKED" },
                    exe.as_deref().unwrap_or("?")
                );
            }
            std::thread::sleep(POLL);
        });
    });
    ALLOWED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_blocks_terminals_but_not_the_game() {
        let p = Policy::Block(DEFAULT_BLOCK.iter().map(|s| s.to_string()).collect());
        assert!(!allows(&p, Some("windowsterminal.exe")));
        assert!(allows(&p, Some("rpcs3.exe")));
        assert!(allows(&p, None));
    }

    #[test]
    fn report_path_check_is_an_atomic_load() {
        let _ = input_allowed(); // start the poller
        let t = std::time::Instant::now();
        let mut n = 0u32;
        for _ in 0..1_000_000 {
            n += input_allowed() as u32;
        }
        let per = t.elapsed().as_nanos() / 1_000_000;
        assert!(per < 200, "input_allowed() took {per} ns/call");
        assert!(n > 0);
    }

    #[test]
    fn only_list_is_strict() {
        let p = Policy::Only(vec!["rpcs3.exe".into()]);
        assert!(allows(&p, Some("rpcs3.exe")));
        assert!(!allows(&p, Some("chrome.exe")));
        assert!(!allows(&p, None));
    }
}
