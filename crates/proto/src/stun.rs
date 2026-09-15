//! Minimal STUN binding request/response (RFC 5389) for endpoint discovery.
//!
//! Used to learn what public `IP:port` a NAT assigns to a specific local UDP
//! socket. That is the fact hole punching is built on: if both peers know each
//! other's public endpoint and both send first, each outbound packet opens the
//! return path for the other, and neither side has to accept an unsolicited
//! inbound connection — which is exactly what a locked-down gateway refuses.
//!
//! Deliberately hand-rolled rather than pulled from the `webrtc` stack: the
//! discovery has to bind *the same port WireGuard uses*, and the parsing is
//! small enough to unit test against real byte layouts with no network at all.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Every STUN message carries this, and a response that does not is not ours.
pub const MAGIC_COOKIE: u32 = 0x2112_A442;

const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const HEADER_LEN: usize = 20;

/// 96-bit transaction id. Responses must echo it, which is what lets us ignore
/// stray datagrams arriving on a socket that is also carrying WireGuard.
pub type TxId = [u8; 12];

/// Encode a binding request. No attributes — we only want the reflexive address.
pub fn binding_request(tx: &TxId) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(&BINDING_REQUEST.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // length: no attributes
    out.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    out.extend_from_slice(tx);
    out
}

/// Extract our public endpoint from a binding success response.
///
/// Returns `None` for anything that is not a success response echoing `tx` —
/// wrong type, wrong cookie, wrong transaction, truncated, or carrying no
/// address attribute. A caller must never treat "parsed nothing" as success.
pub fn parse_binding_response(tx: &TxId, buf: &[u8]) -> Option<SocketAddr> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type != BINDING_SUCCESS {
        return None;
    }
    let attrs_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let cookie = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    if cookie != MAGIC_COOKIE {
        return None;
    }
    if &buf[8..20] != tx.as_slice() {
        return None;
    }
    if buf.len() < HEADER_LEN + attrs_len {
        return None;
    }

    let mut i = HEADER_LEN;
    let end = HEADER_LEN + attrs_len;
    // Prefer XOR-MAPPED-ADDRESS: plain MAPPED-ADDRESS is mangled by NATs that
    // rewrite anything resembling an address in the payload, which is the very
    // reason the XOR form exists.
    let mut plain: Option<SocketAddr> = None;
    while i + 4 <= end {
        let attr_type = u16::from_be_bytes([buf[i], buf[i + 1]]);
        let attr_len = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
        let val_start = i + 4;
        let val_end = val_start.checked_add(attr_len)?;
        if val_end > end {
            return None;
        }
        let value = &buf[val_start..val_end];
        match attr_type {
            ATTR_XOR_MAPPED_ADDRESS => {
                if let Some(a) = decode_address(value, tx, true) {
                    return Some(a);
                }
            }
            ATTR_MAPPED_ADDRESS => {
                if plain.is_none() {
                    plain = decode_address(value, tx, false);
                }
            }
            _ => {}
        }
        // Attributes are padded to a 4-byte boundary; skipping the padding is
        // what keeps a multi-attribute response parseable.
        i = val_end + ((4 - (attr_len % 4)) % 4);
    }
    plain
}

fn decode_address(value: &[u8], tx: &TxId, xor: bool) -> Option<SocketAddr> {
    if value.len() < 4 {
        return None;
    }
    let family = value[1];
    let raw_port = u16::from_be_bytes([value[2], value[3]]);
    let port = if xor {
        raw_port ^ (MAGIC_COOKIE >> 16) as u16
    } else {
        raw_port
    };
    match family {
        0x01 => {
            if value.len() < 8 {
                return None;
            }
            let mut octets = [0u8; 4];
            octets.copy_from_slice(&value[4..8]);
            if xor {
                let cookie = MAGIC_COOKIE.to_be_bytes();
                for (o, c) in octets.iter_mut().zip(cookie.iter()) {
                    *o ^= c;
                }
            }
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(octets)), port))
        }
        0x02 => {
            if value.len() < 20 {
                return None;
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&value[4..20]);
            if xor {
                // IPv6 is XORed with cookie || transaction id.
                let mut key = [0u8; 16];
                key[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
                key[4..].copy_from_slice(tx);
                for (o, k) in octets.iter_mut().zip(key.iter()) {
                    *o ^= k;
                }
            }
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port))
        }
        _ => None,
    }
}

/// Two observations of our own public endpoint, taken against *different* STUN
/// servers from the same local port.
///
/// If they disagree the NAT is assigning a fresh mapping per destination —
/// symmetric — and the port learned from one server is useless for talking to a
/// peer. Hole punching cannot work there, and detecting it early matters: the
/// alternative is a user watching a doomed handshake retry for 30 seconds
/// before falling back to a relay that would have worked immediately.
pub fn is_symmetric_nat(a: SocketAddr, b: SocketAddr) -> bool {
    a.port() != b.port() || a.ip() != b.ip()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx() -> TxId {
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    }

    #[test]
    fn binding_request_is_a_well_formed_header() {
        let req = binding_request(&tx());
        assert_eq!(req.len(), HEADER_LEN);
        assert_eq!(u16::from_be_bytes([req[0], req[1]]), BINDING_REQUEST);
        assert_eq!(u16::from_be_bytes([req[2], req[3]]), 0, "no attributes");
        assert_eq!(
            u32::from_be_bytes([req[4], req[5], req[6], req[7]]),
            MAGIC_COOKIE
        );
        assert_eq!(&req[8..20], &tx());
    }

    /// Build a success response carrying one XOR-MAPPED-ADDRESS for IPv4.
    fn xor_v4_response(tx: &TxId, ip: [u8; 4], port: u16) -> Vec<u8> {
        let cookie = MAGIC_COOKIE.to_be_bytes();
        let mut value = vec![0u8, 0x01];
        value.extend_from_slice(&(port ^ (MAGIC_COOKIE >> 16) as u16).to_be_bytes());
        for (i, o) in ip.iter().enumerate() {
            value.push(o ^ cookie[i]);
        }
        let mut out = Vec::new();
        out.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
        out.extend_from_slice(&((4 + value.len()) as u16).to_be_bytes());
        out.extend_from_slice(&cookie);
        out.extend_from_slice(tx);
        out.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(&value);
        out
    }

    #[test]
    fn decodes_xor_mapped_ipv4() {
        let buf = xor_v4_response(&tx(), [203, 0, 113, 7], 51820);
        let got = parse_binding_response(&tx(), &buf).expect("parsed");
        assert_eq!(got, "203.0.113.7:51820".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn rejects_a_response_for_someone_elses_transaction() {
        // A socket carrying WireGuard traffic will see datagrams that are not
        // ours; accepting one would hand back a wrong public endpoint.
        let buf = xor_v4_response(&tx(), [203, 0, 113, 7], 51820);
        let other = [9u8; 12];
        assert!(parse_binding_response(&other, &buf).is_none());
    }

    #[test]
    fn rejects_a_bad_magic_cookie() {
        let mut buf = xor_v4_response(&tx(), [203, 0, 113, 7], 51820);
        buf[4] ^= 0xFF;
        assert!(parse_binding_response(&tx(), &buf).is_none());
    }

    #[test]
    fn rejects_truncated_messages() {
        let buf = xor_v4_response(&tx(), [203, 0, 113, 7], 51820);
        for cut in [0usize, 4, 19, 21] {
            assert!(parse_binding_response(&tx(), &buf[..cut.min(buf.len())]).is_none());
        }
    }

    #[test]
    fn rejects_an_attribute_length_past_the_end() {
        // A malformed or hostile response must not panic the client.
        let mut buf = xor_v4_response(&tx(), [203, 0, 113, 7], 51820);
        let n = buf.len();
        buf[n - 10] = 0xFF; // attribute length byte
        assert!(parse_binding_response(&tx(), &buf).is_none());
    }

    #[test]
    fn decodes_xor_mapped_ipv6() {
        let tx = tx();
        let ip: Ipv6Addr = "2001:db8::1".parse().unwrap();
        let port = 51820u16;
        let mut key = [0u8; 16];
        key[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
        key[4..].copy_from_slice(&tx);
        let mut enc = ip.octets();
        for (o, k) in enc.iter_mut().zip(key.iter()) {
            *o ^= k;
        }
        let mut value = vec![0u8, 0x02];
        value.extend_from_slice(&(port ^ (MAGIC_COOKIE >> 16) as u16).to_be_bytes());
        value.extend_from_slice(&enc);

        let mut buf = Vec::new();
        buf.extend_from_slice(&BINDING_SUCCESS.to_be_bytes());
        buf.extend_from_slice(&((4 + value.len()) as u16).to_be_bytes());
        buf.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        buf.extend_from_slice(&tx);
        buf.extend_from_slice(&ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        buf.extend_from_slice(&(value.len() as u16).to_be_bytes());
        buf.extend_from_slice(&value);

        let got = parse_binding_response(&tx, &buf).expect("parsed");
        assert_eq!(got, SocketAddr::new(IpAddr::V6(ip), port));
    }

    #[test]
    fn symmetric_nat_is_detected_by_a_differing_mapping() {
        let a: SocketAddr = "203.0.113.7:51820".parse().unwrap();
        let same: SocketAddr = "203.0.113.7:51820".parse().unwrap();
        let diff_port: SocketAddr = "203.0.113.7:51999".parse().unwrap();
        assert!(!is_symmetric_nat(a, same));
        assert!(is_symmetric_nat(a, diff_port));
    }
}
