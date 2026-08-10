//! Bring up the WireGuard tunnel carried in a join link.
//!
//! The invite ships the friend's whole config (`wg=`), so joining a direct
//! tunnel is one paste instead of a file transfer. This module turns that text
//! into a live interface.
//!
//! Everything here is best-effort by design: if the tunnel cannot be raised the
//! client must still connect over whatever path the invite already described.
//! A failed tunnel is a slower session, not a broken one.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

/// Interface name for the tunnel we manage. Matches what setup-wireguard.sh
/// and enable-wireguard.ps1 use, so we never end up with two rival tunnels.
const IFACE: &str = "couchlink";

/// Outcome of trying to raise the tunnel, so callers can report honestly
/// instead of assuming success.
#[derive(Debug, PartialEq, Eq)]
pub enum TunnelState {
    /// A handshake was observed — the tunnel is genuinely carrying traffic.
    Up,
    /// Already up before we did anything.
    AlreadyUp,
    /// Config written but no handshake. The peer is unreachable on that
    /// endpoint; the caller should fall back rather than wait.
    NoHandshake,
    /// Tooling missing or the platform is unsupported here.
    Unavailable(String),
}

/// Where the config gets written. Under the user's config dir, not /etc, so
/// this needs no privileges to *write* — only to bring the interface up.
pub fn conf_path() -> PathBuf {
    dirs_config()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("couchlink")
        .join(format!("{IFACE}.conf"))
}

fn dirs_config() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Some(PathBuf::from(x));
        }
    }
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config"))
}

/// Write the config with 0600 permissions — it contains a private key.
pub fn write_conf(conf: &str, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let body = crate::invite::wireguard_conf_file(conf);
    std::fs::write(path, body).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // A WireGuard private key must not be world-readable.
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// True when `wg show` reports a peer with a recent, non-zero handshake.
///
/// "The interface exists" is not evidence the tunnel works — that exact
/// assumption is what let the host advertise an unreachable 10.66.0.x address
/// while skipping working fallbacks. Only a handshake counts.
pub fn has_handshake(iface: &str) -> bool {
    let out = match Command::new("wg")
        .args(["show", iface, "latest-handshakes"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };
    parse_latest_handshakes(&String::from_utf8_lossy(&out.stdout))
}

/// `wg show <if> latest-handshakes` prints `<pubkey>\t<unix-seconds>` per peer.
/// A zero timestamp means "never handshaked", which is the case we must not
/// mistake for success.
pub fn parse_latest_handshakes(stdout: &str) -> bool {
    stdout.lines().any(|line| {
        line.split_whitespace()
            .nth(1)
            .and_then(|t| t.parse::<i64>().ok())
            .is_some_and(|t| t > 0)
    })
}

/// Write the config and try to raise the tunnel, waiting briefly for a
/// handshake before declaring anything.
pub fn ensure_up(conf: &str) -> TunnelState {
    if has_handshake(IFACE) {
        info!("WireGuard tunnel {IFACE} already up");
        return TunnelState::AlreadyUp;
    }

    let path = conf_path();
    if let Err(e) = write_conf(conf, &path) {
        return TunnelState::Unavailable(format!("could not write config: {e:#}"));
    }

    if which("wg-quick").is_none() {
        return TunnelState::Unavailable(
            "wg-quick not found — install wireguard-tools to use the direct tunnel".into(),
        );
    }

    // wg-quick needs root to create an interface. Try without a password
    // prompt first: a client that silently blocks on a hidden sudo prompt is
    // worse than one that reports it cannot raise the tunnel.
    // Capture rather than inherit: `sudo -n` prints "a password is required" to
    // stderr before we get to say anything, which reads like a crash right
    // above our own clearer message.
    let status = Command::new("sudo")
        .args(["-n", "wg-quick", "up"])
        .arg(&path)
        .output();
    match status {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let why = String::from_utf8_lossy(&o.stderr);
            let why = why.trim();
            if why.contains("password is required") || why.contains("may not run") {
                return TunnelState::Unavailable(format!(
                    "needs root — run: sudo wg-quick up {}",
                    path.display()
                ));
            }
            return TunnelState::Unavailable(format!(
                "wg-quick up failed: {}",
                if why.is_empty() { "unknown error" } else { why }
            ));
        }
        Err(e) => return TunnelState::Unavailable(format!("wg-quick failed to run: {e}")),
    }

    // A handshake is not instant; give it a moment before judging.
    for _ in 0..10 {
        if has_handshake(IFACE) {
            info!("WireGuard tunnel {IFACE} up (handshake confirmed)");
            return TunnelState::Up;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    warn!(
        "WireGuard config applied but no handshake — the host endpoint is not reachable from here"
    );
    TunnelState::NoHandshake
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(bin))
            .find(|p| p.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_never_handshaked_peer_is_not_up() {
        // Regression: treating "the interface exists" as success is what let a
        // dead tunnel beat working fallbacks.
        let out = "AbCdEf0123456789AbCdEf0123456789AbCdEf01234=\t0\n";
        assert!(!parse_latest_handshakes(out));
    }

    #[test]
    fn a_real_handshake_counts() {
        let out = "AbCdEf0123456789AbCdEf0123456789AbCdEf01234=\t1754500000\n";
        assert!(parse_latest_handshakes(out));
    }

    #[test]
    fn one_live_peer_among_dead_ones_counts() {
        let out = "keyA=\t0\nkeyB=\t1754500000\n";
        assert!(parse_latest_handshakes(out));
    }

    #[test]
    fn empty_output_is_not_up() {
        assert!(!parse_latest_handshakes(""));
        assert!(!parse_latest_handshakes("\n"));
    }

    #[test]
    fn conf_written_is_newline_terminated_and_complete() {
        let dir = std::env::temp_dir().join(format!("cl-wg-test-{}", std::process::id()));
        let path = dir.join("t.conf");
        let conf = "[Interface]\nAddress = 10.66.0.2/24\n\n[Peer]\nEndpoint = [2603::1]:51820";
        write_conf(conf, &path).unwrap();
        let got = std::fs::read_to_string(&path).unwrap();
        assert!(got.ends_with('\n'));
        assert!(got.contains("Endpoint = [2603::1]:51820"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
