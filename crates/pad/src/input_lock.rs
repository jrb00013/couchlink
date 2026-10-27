//! Host-side "input lock": while engaged, every viewer pad frame is replaced by a neutral
//! one before it reaches the virtual controller, so viewers cannot drive anything on the
//! host (a virtual XInput pad is a real controller to Windows, and XAML apps such as
//! Windows Terminal answer D-pad/A/B by moving focus around their tab strip).
//!
//! The lock is a file, so a shell script, the TUI or anything else can flip it without a
//! round trip through the host: present = locked, absent = unlocked. A background thread
//! re-reads it every [`POLL_MS`] ms, so the per-frame check is a single atomic load.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use tracing::info;

/// How often the lock file is re-read (off the frame path).
const POLL_MS: u64 = 150;

static LOCKED: AtomicBool = AtomicBool::new(false);

/// Lock file location: `COUCHLINK_INPUT_LOCK_FILE`, else `%LOCALAPPDATA%\couchlink\input.lock`
/// on Windows, else `$XDG_RUNTIME_DIR/couchlink-input.lock` (falling back to `/tmp`).
/// `scripts/input-lock.sh` resolves the same path.
pub fn lock_path() -> PathBuf {
    if let Some(p) = std::env::var_os("COUCHLINK_INPUT_LOCK_FILE") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        if let Some(base) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(base).join("couchlink").join("input.lock");
        }
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    dir.join("couchlink-input.lock")
}

/// Whether viewer input is currently blocked. One relaxed atomic load: the lock file is
/// watched by a background thread (started on first call), so the input path does no I/O.
pub fn is_locked() -> bool {
    static POLLER: OnceLock<()> = OnceLock::new();
    POLLER.get_or_init(|| {
        refresh();
        let _ = std::thread::Builder::new().name("input-lock-poll".into()).spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
            refresh();
        });
    });
    LOCKED.load(Ordering::Relaxed)
}

fn refresh() {
    let locked = lock_path().exists();
    if LOCKED.swap(locked, Ordering::Relaxed) != locked {
        if locked {
            info!("input lock ENGAGED - viewer input is blocked ({})", lock_path().display());
        } else {
            info!("input lock released - viewer input is live again");
        }
    }
}

/// Engage or release the lock (creates / removes the lock file).
pub fn set_locked(locked: bool) -> std::io::Result<()> {
    let path = lock_path();
    if locked {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, b"locked\n")
    } else {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("couchlink-lock-test-{}", std::process::id()));
        let file = dir.join("input.lock");
        std::env::set_var("COUCHLINK_INPUT_LOCK_FILE", &file);
        assert_eq!(lock_path(), file);

        set_locked(false).unwrap();
        refresh();
        assert!(!is_locked());

        set_locked(true).unwrap();
        refresh();
        assert!(is_locked());

        set_locked(false).unwrap();
        refresh();
        assert!(!is_locked());
        set_locked(false).unwrap(); // releasing twice is fine
        let _ = std::fs::remove_dir_all(&dir);
    }
}
