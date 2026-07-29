//! Client-side ICE reachability helpers.
//!
//! Friends on WSL (or any nested NAT) are not inbound-reachable on their eth0
//! address. Production WebRTC covers that with STUN + TURN (UDP and TCP), and
//! optional NAT 1:1 host candidates pointing at a Windows LAN IP so same-LAN
//! peers can punch without waiting on relay.

use std::process::Command;

/// True when running under WSL1/WSL2.
pub fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| v.to_ascii_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

/// Register both UDP and TCP TURN URLs when the invite only lists one.
///
/// WSL and many carrier NATs drop or rewrite UDP; coturn already listens on TCP
/// 3478 (`scripts/start-turn.sh` opens both). Browsers and webrtc-rs pick whichever
/// candidate pair connects.
pub fn expand_turn_urls(url: &str) -> Vec<String> {
    let base = url.trim();
    if base.is_empty() {
        return Vec::new();
    }
    let mut out = vec![base.to_string()];
    if !base.to_ascii_lowercase().contains("transport=tcp") {
        let sep = if base.contains('?') { '&' } else { '?' };
        out.push(format!("{base}{sep}transport=tcp"));
    }
    out
}

/// Best-effort IPs to advertise as ICE host candidates via NAT 1:1.
///
/// On WSL, the Linux eth0 address is only useful inside the Hyper-V switch.
/// Prefer the Windows host's physical LAN IPv4 so a peer on the same Wi‑Fi can
/// reach the client without TURN. Internet peers still need STUN/TURN.
pub fn discover_ice_ips(explicit: Vec<String>) -> Vec<String> {
    let mut ips: Vec<String> = explicit
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if !ips.is_empty() {
        return ips;
    }
    if is_wsl() {
        if let Some(ip) = windows_lan_ipv4() {
            ips.push(ip);
        }
    }
    ips
}

fn windows_lan_ipv4() -> Option<String> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.PrefixOrigin -ne 'WellKnown' -and $_.IPAddress -notlike '169.254.*' -and $_.InterfaceAlias -notmatch 'Loopback|vEthernet|WSL|Hyper-V|Docker' } | Select-Object -ExpandProperty IPAddress -First 1)",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let ip = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches(|c: char| c == '\r' || c == '\n')
        .to_string();
    if ip.is_empty() || !ip.contains('.') {
        return None;
    }
    Some(ip)
}

/// Signaling looks remote (not loopback) — TURN should be present for NAT.
pub fn signaling_needs_turn(signaling: &str) -> bool {
    let lower = signaling.to_ascii_lowercase();
    !(lower.contains("127.0.0.1")
        || lower.contains("localhost")
        || lower.contains("[::1]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_turn_with_tcp() {
        let urls = expand_turn_urls("turn:203.0.113.10:3478");
        assert_eq!(
            urls,
            vec![
                "turn:203.0.113.10:3478".to_string(),
                "turn:203.0.113.10:3478?transport=tcp".to_string(),
            ]
        );
    }

    #[test]
    fn does_not_duplicate_tcp() {
        let urls = expand_turn_urls("turn:1.2.3.4:3478?transport=tcp");
        assert_eq!(urls, vec!["turn:1.2.3.4:3478?transport=tcp".to_string()]);
    }

    #[test]
    fn local_signaling_skips_turn_requirement() {
        assert!(!signaling_needs_turn("ws://127.0.0.1:8443/ws"));
        assert!(signaling_needs_turn("ws://203.0.113.10:8443/ws"));
    }
}
