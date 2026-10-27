//! Host-side "input lock": while engaged, every viewer pad frame is replaced by a neutral
//! one before it reaches the virtual controller, so viewers cannot drive anything on the
//! host (a virtual XInput pad is a real controller to Windows, and XAML apps such as
//! Windows Terminal answer D-pad/A/B by moving focus around their tab strip).
//!
//! The lock is a file, so a shell script, the TUI or anything else can flip it without a
//! round trip through the host: present = locked, absent = unlocked. The host polls it
//! at most every [`POLL_MS`] ms, so the check is cheap enough for the per-frame path.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use tracing::info;

/// How often the lock file is re-read on the frame path.
const POLL_MS: u64 = 150;

static LOCKED: AtomicBool = AtomicBool::new(false);
static LAST_POLL_MS: AtomicU64 = AtomicU64::new(u64::MAX);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

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

/// Whether viewer input is currently blocked. Cached for [`POLL_MS`].
pub fn is_locked() -> bool {
    let now = epoch().elapsed().as_millis() as u64;
    let last = LAST_POLL_MS.load(Ordering::Relaxed);
    if last != u64::MAX && now.saturating_sub(last) < POLL_MS {
        return LOCKED.load(Ordering::Relaxed);
    }
    LAST_POLL_MS.store(now, Ordering::Relaxed);
    let locked = lock_path().exists();
    if LOCKED.swap(locked, Ordering::Relaxed) != locked {
        if locked {
            info!("input lock ENGAGED - viewer input is blocked ({})", lock_path().display());
        } else {
            info!("input lock released - viewer input is live again");
        }
    }
    locked
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
        LAST_POLL_MS.store(u64::MAX, Ordering::Relaxed);
        assert!(!is_locked());

        set_locked(true).unwrap();
        LAST_POLL_MS.store(u64::MAX, Ordering::Relaxed);
        assert!(is_locked());

        set_locked(false).unwrap();
        LAST_POLL_MS.store(u64::MAX, Ordering::Relaxed);
        assert!(!is_locked());
        set_locked(false).unwrap(); // releasing twice is fine
        let _ = std::fs::remove_dir_all(&dir);
    }
}
