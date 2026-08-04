//! Match the virtual pad and the emulator binding to the controller the player
//! is actually holding.
//!
//! The browser Gamepad API normalises every pad, so `PadFrame` looks identical
//! whether the player holds an Xbox pad or a DualSense. The player therefore
//! announces its family over signaling (`PadInfo`), and this module reconciles
//! two things against it: which virtual device the companion presents, and
//! which device the emulator has bound to that player slot. Getting either
//! wrong drops every button with no error anywhere — the failure is silent on
//! the host, in the browser, and in the emulator.

use std::path::PathBuf;
use std::process::Command;

use tracing::{info, warn};

/// Virtual-pad backend and emulator handler for a reported controller family.
///
/// `generic` maps to Xbox because XInput is the one handler that is present on
/// every Windows emulator build and needs no vendor driver.
fn backend_for(kind: &str) -> &'static str {
    match kind {
        "dualsense" => "ds4",
        "xbox" | "generic" => "xbox360",
        _ => "xbox360",
    }
}

/// Repo root, so the helper scripts can be found from a binary in `target/`.
fn repo_root() -> Option<PathBuf> {
    if let Ok(r) = std::env::var("COUCHLINK_ROOT") {
        let p = PathBuf::from(r);
        if p.join("scripts").is_dir() {
            return Some(p);
        }
    }
    // target/release/couchlink-host -> repo root
    let exe = std::env::current_exe().ok()?;
    let root = exe.parent()?.parent()?.parent()?.to_path_buf();
    root.join("scripts").is_dir().then_some(root)
}

/// Reconcile the virtual pad + emulator binding with `kind`.
///
/// Best-effort by design: every failure here still leaves video streaming, so
/// nothing in this path is allowed to take the session down.
pub fn apply(kind: &str, id: &str) {
    let backend = backend_for(kind);
    let Some(root) = repo_root() else {
        warn!("pad_info {kind}: repo root not found — emulator binding unchanged");
        return;
    };

    info!("player pad is {kind} ({id}) — virtual pad backend {backend}");

    // Companion first: the emulator binding names the device the companion
    // presents, so rebinding before it exists would point at nothing.
    run(&root, "scripts/ensure-ds-vhid.sh", backend);
    run(&root, "scripts/link-emulator-pad.sh", backend);
}

fn run(root: &PathBuf, script: &str, backend: &str) {
    let path = root.join(script);
    if !path.is_file() {
        warn!("{script} missing — skipped");
        return;
    }
    match Command::new("bash")
        .arg(&path)
        .current_dir(root)
        .env("COUCHLINK_DS_VHID_BACKEND", backend)
        .output()
    {
        Ok(out) if out.status.success() => {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if !line.trim().is_empty() {
                    info!("{}", line.trim());
                }
            }
        }
        Ok(out) => warn!(
            "{script} exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => warn!("{script} failed to run: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xbox_and_generic_use_xinput_backend() {
        assert_eq!(backend_for("xbox"), "xbox360");
        // Generic pads must not land on a Sony backend: XInput is the handler
        // that exists without a vendor driver.
        assert_eq!(backend_for("generic"), "xbox360");
        assert_eq!(backend_for("something-new"), "xbox360");
    }

    #[test]
    fn dualsense_uses_a_sony_backend() {
        assert_eq!(backend_for("dualsense"), "ds4");
    }
}
