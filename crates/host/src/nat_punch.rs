//! Live STUN-based endpoint discovery for the WireGuard invite.
//!
//! `scripts/setup-wireguard.sh` already resolves an `Endpoint` for the player
//! config — `COUCHLINK_WG_ENDPOINT`, else `COUCHLINK_PUBLIC_IP`, else a public
//! IP lookup — but all three assume a stable, forwarded, or mesh-routed
//! address. Behind a plain home NAT with no port forward that assumption
//! fails: the router's own public IP is not necessarily what a peer must dial
//! to reach *this* WireGuard socket, and the actual NAT-assigned external port
//! is often different from `ListenPort` entirely.
//!
//! This asks the NAT itself, from the exact socket WireGuard is about to use,
//! at session start rather than baking a guess into a file ahead of time. It
//! is strictly additive: gated behind `COUCHLINK_WG_PUNCH=1`, and any failure
//! — no network, symmetric NAT, unparseable config — falls straight back to
//! whatever the static config already said. A tunnel that stays on the old
//! path is a slower session, never a broken one.

use couchlink_proto::stun::{binding_request, is_symmetric_nat, parse_binding_response, TxId};
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;
use tracing::{debug, warn};

/// Two independent providers, matching the verification in the `stun` module
/// itself — a single server can't distinguish "consistent mapping" from
/// "asked the same question twice".
pub const DEFAULT_STUN_SERVERS: [&str; 2] =
    ["stun.l.google.com:19302", "stun.cloudflare.com:3478"];

const RECV_TIMEOUT: Duration = Duration::from_millis(800);

/// Query one STUN server for the address it observes for `sock`.
fn query(sock: &UdpSocket, server: &str) -> Option<SocketAddr> {
    let tx: TxId = rand_tx_id();
    let req = binding_request(&tx);
    sock.send_to(&req, server).ok()?;
    let mut buf = [0u8; 512];
    let (n, from) = sock.recv_from(&mut buf).ok()?;
    let got = parse_binding_response(&tx, &buf[..n])?;
    debug!(%server, %from, %got, "stun binding response");
    Some(got)
}

fn rand_tx_id() -> TxId {
    // No crypto requirement — this only has to be unlikely to collide with
    // whatever else briefly shares the socket, not unguessable.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ (std::process::id() as u64) << 32;
    let mut tx = [0u8; 12];
    let bytes = seed.to_le_bytes();
    for (i, b) in tx.iter_mut().enumerate() {
        *b = bytes[i % 8].wrapping_add(i as u8);
    }
    tx
}

/// Bind `local_port` (the same port WireGuard will listen on) and ask two
/// STUN servers what public endpoint the NAT assigns it.
///
/// `None` covers every failure mode on purpose: no route to either server, a
/// symmetric NAT (the two answers disagree — hole punching cannot work
/// there), or the port already in use by the running `wg-quick` interface. A
/// caller must never distinguish these and must always be ready to fall back.
pub fn discover_public_endpoint(local_port: u16, servers: &[&str]) -> Option<SocketAddr> {
    let sock = UdpSocket::bind(("0.0.0.0", local_port)).ok()?;
    sock.set_read_timeout(Some(RECV_TIMEOUT)).ok()?;

    let mut seen: Option<SocketAddr> = None;
    for server in servers {
        let Some(addr) = query(&sock, server) else {
            continue;
        };
        match seen {
            None => seen = Some(addr),
            Some(prior) if is_symmetric_nat(prior, addr) => {
                warn!(
                    %prior, %addr,
                    "NAT mapping differs by destination — symmetric NAT, hole punching not viable"
                );
                return None;
            }
            Some(_) => {}
        }
    }
    seen
}

/// Replace (or insert) the `Endpoint = ` line under `[Peer]` in a WireGuard
/// config with a freshly discovered address.
///
/// Pure text transform — kept separate from the network call above so the
/// substitution logic is testable without a socket, matching how `stun.rs`
/// tests its own byte parsing with no network at all.
pub fn rewrite_wg_endpoint(conf: &str, endpoint: SocketAddr) -> String {
    let literal = match endpoint {
        SocketAddr::V4(a) => format!("Endpoint = {}:{}", a.ip(), a.port()),
        SocketAddr::V6(a) => format!("Endpoint = [{}]:{}", a.ip(), a.port()),
    };
    let mut out = String::with_capacity(conf.len() + literal.len());
    let mut replaced = false;
    let mut in_peer = false;
    for line in conf.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("[Peer]") {
            in_peer = true;
        } else if trimmed.starts_with('[') {
            in_peer = false;
        }
        if in_peer && trimmed.starts_with("Endpoint") && trimmed.contains('=') {
            out.push_str(&literal);
            out.push('\n');
            replaced = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        // No existing Endpoint line — nothing to graft onto safely without
        // knowing where [Peer] starts; leave the config untouched rather than
        // guess at placement.
        return conf.to_string();
    }
    out
}

/// Pull the port out of the static config's existing `Endpoint = host:port`
/// line — the player conf has no `ListenPort` of its own (only the host-side
/// `wg0-host.conf` does, which this process never reads), but
/// `setup-wireguard.sh` always writes `Endpoint = ${ENDPOINT_LITERAL}:${LISTEN_PORT}`,
/// so the port half of that line already carries the value we need to bind.
pub fn endpoint_port_from_conf(conf: &str) -> Option<u16> {
    let mut in_peer = false;
    for line in conf.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("[Peer]") {
            in_peer = true;
        } else if trimmed.starts_with('[') {
            in_peer = false;
        }
        if in_peer && trimmed.starts_with("Endpoint") {
            let (_, rest) = trimmed.split_once('=')?;
            let rest = rest.trim();
            // Either `host:port` or `[v6::addr]:port`.
            let port_str = rest.rsplit(':').next()?;
            return port_str.parse().ok();
        }
    }
    None
}

/// Best-effort: discover this host's real NAT-punched endpoint for
/// `local_port` and splice it into `conf`. `None` on any failure — callers
/// keep using the original `conf` unchanged.
pub fn discover_and_rewrite(conf: &str, local_port: u16, servers: &[&str]) -> Option<String> {
    let endpoint = discover_public_endpoint(local_port, servers)?;
    Some(rewrite_wg_endpoint(conf, endpoint))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "[Interface]\nAddress = 10.66.0.2/24\nPrivateKey = PLAYER_PRIVATE_KEY\n\n[Peer]\nPublicKey = HOST_PUBLIC_KEY\nEndpoint = HOST_PUBLIC_IP:51820\nAllowedIPs = 10.66.0.0/24\nPersistentKeepalive = 25\n";

    #[test]
    fn rewrites_ipv4_endpoint_under_peer() {
        let ep: SocketAddr = "203.0.113.7:58783".parse().unwrap();
        let out = rewrite_wg_endpoint(EXAMPLE, ep);
        assert!(out.contains("Endpoint = 203.0.113.7:58783"));
        assert!(!out.contains("HOST_PUBLIC_IP"));
        // Everything else in the file must survive untouched.
        assert!(out.contains("PublicKey = HOST_PUBLIC_KEY"));
        assert!(out.contains("PersistentKeepalive = 25"));
    }

    #[test]
    fn rewrites_ipv6_endpoint_bracketed() {
        let ep: SocketAddr = "[2001:db8::7]:51820".parse().unwrap();
        let out = rewrite_wg_endpoint(EXAMPLE, ep);
        assert!(out.contains("Endpoint = [2001:db8::7]:51820"));
    }

    #[test]
    fn a_config_with_no_endpoint_line_is_left_untouched() {
        let conf = "[Interface]\nAddress = 10.66.0.2/24\n\n[Peer]\nPublicKey = X\n";
        let ep: SocketAddr = "203.0.113.7:58783".parse().unwrap();
        assert_eq!(rewrite_wg_endpoint(conf, ep), conf);
    }

    #[test]
    fn only_the_peer_sections_endpoint_line_is_touched() {
        // A line that merely contains the word "Endpoint" outside [Peer] (or
        // in a comment) must not be rewritten.
        let conf = "[Interface]\n# Endpoint = should-not-touch\nAddress = 10.66.0.2/24\n\n[Peer]\nEndpoint = OLD:1\n";
        let ep: SocketAddr = "203.0.113.7:58783".parse().unwrap();
        let out = rewrite_wg_endpoint(conf, ep);
        assert!(out.contains("# Endpoint = should-not-touch"));
        assert!(out.contains("Endpoint = 203.0.113.7:58783"));
        assert!(!out.contains("OLD:1"));
    }

    #[test]
    fn extracts_port_from_the_static_endpoint_line() {
        assert_eq!(endpoint_port_from_conf(EXAMPLE), Some(51820));
    }

    #[test]
    fn extracts_port_from_a_bracketed_ipv6_endpoint() {
        let conf = "[Peer]\nEndpoint = [2603::1]:51821\n";
        assert_eq!(endpoint_port_from_conf(conf), Some(51821));
    }

    #[test]
    fn missing_endpoint_line_yields_none() {
        let conf = "[Interface]\nAddress = 10.66.0.2/24\n\n[Peer]\nPublicKey = X\n";
        assert_eq!(endpoint_port_from_conf(conf), None);
    }

    #[test]
    fn discovery_against_an_unreachable_server_is_none_not_a_panic() {
        // Port 0 asks the OS for an ephemeral port so this never collides with
        // a real listener; 192.0.2.0/24 is TEST-NET-1 (RFC 5737) — guaranteed
        // unroutable, so the query times out instead of hanging on real I/O.
        let got = discover_public_endpoint(0, &["192.0.2.1:19302"]);
        assert!(got.is_none());
    }

    #[test]
    fn rand_tx_ids_are_the_right_length_and_vary() {
        let a = rand_tx_id();
        std::thread::sleep(Duration::from_millis(2));
        let b = rand_tx_id();
        assert_eq!(a.len(), 12);
        assert_ne!(a, b, "two calls a moment apart must not collide");
    }
}
