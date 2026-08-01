//! Parse browser-style join links (same query params as web/src/App.tsx).

use anyhow::{Context, Result};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedInvite {
    pub signaling: String,
    pub session_id: String,
    pub pin: String,
    pub turn_url: Option<String>,
    pub turn_user: Option<String>,
    pub turn_pass: Option<String>,
    /// Optional mesh hint from host (`headscale` / `tailscale` / `wireguard`).
    pub mesh: Option<String>,
    /// Headscale control-plane URL (`hs=`).
    pub hs_url: Option<String>,
    /// Headscale / Tailscale preauth key (`tskey=`).
    pub ts_authkey: Option<String>,
}

pub fn parse_join_url(raw: &str) -> Result<ParsedInvite> {
    let trimmed = raw.trim();
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let url = Url::parse(&with_scheme).context("invalid join URL")?;

    let session_id = query(&url, &["s", "session"]).context("join URL missing session (?s= or ?session=)")?;
    let pin = query(&url, &["p", "pin"]).context("join URL missing PIN (?p= or ?pin=)")?;

    let signaling = query(&url, &["ws", "signaling"]).unwrap_or_else(|| default_signaling_ws(&url));

    let turn_url = query(&url, &["turn"]);
    let turn_user = query(&url, &["turnu"]);
    let turn_pass = query(&url, &["turnp"]);
    let mesh = query(&url, &["mesh"]);
    let hs_url = query(&url, &["hs"]);
    let ts_authkey = query(&url, &["tskey"]);

    Ok(ParsedInvite {
        signaling,
        session_id,
        pin,
        turn_url,
        turn_user,
        turn_pass,
        mesh,
        hs_url,
        ts_authkey,
    })
}

/// Waiting-screen field: full join URL **or** `session:pin` / `session/pin`.
pub fn parse_join_input(raw: &str) -> Result<ParsedInvite> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        anyhow::bail!("paste a join URL, or session:pin");
    }
    let looks_like_url = trimmed.contains("://")
        || trimmed.contains('?')
        || trimmed.starts_with("http")
        || trimmed.starts_with("ws");
    if looks_like_url {
        return parse_join_url(trimmed);
    }
    let (session_id, pin) = trimmed
        .split_once(':')
        .or_else(|| trimmed.split_once('/'))
        .map(|(s, p)| (s.trim(), p.trim()))
        .filter(|(s, p)| !s.is_empty() && !p.is_empty())
        .context("expected join URL or session:pin")?;
    Ok(ParsedInvite {
        signaling: "ws://127.0.0.1:8443/ws".into(),
        session_id: session_id.to_string(),
        pin: pin.to_string(),
        turn_url: None,
        turn_user: None,
        turn_pass: None,
        mesh: None,
        hs_url: None,
        ts_authkey: None,
    })
}

/// True when invite carries Headscale control URL + preauth key (or mesh=headscale).
pub fn is_headscale_invite(parsed: &ParsedInvite) -> bool {
    if parsed.mesh.as_deref() == Some("headscale") {
        return true;
    }
    matches!(
        (&parsed.hs_url, &parsed.ts_authkey),
        (Some(h), Some(k)) if !h.is_empty() && !k.is_empty()
    )
}

/// True when the invite targets a Tailscale CGNAT address (100.64.0.0/10).
pub fn is_tailscale_invite(parsed: &ParsedInvite) -> bool {
    if is_headscale_invite(parsed) {
        // Headscale also uses 100.x — prefer the headscale message path.
        return false;
    }
    if parsed.mesh.as_deref() == Some("tailscale") {
        return true;
    }
    host_looks_tailscale(&parsed.signaling)
}

fn host_looks_tailscale(signaling: &str) -> bool {
    let Ok(u) = Url::parse(signaling) else {
        return false;
    };
    let Some(host) = u.host_str() else {
        return false;
    };
    let Ok(ip) = host.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    // Tailscale CGNAT: 100.64.0.0/10
    (ip.octets()[0] == 100) && (ip.octets()[1] & 0xc0) == 64
}

fn query(url: &Url, keys: &[&str]) -> Option<String> {
    for (k, v) in url.query_pairs() {
        if keys.iter().any(|key| *key == k) {
            let s = v.trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

fn default_signaling_ws(page: &Url) -> String {
    let host = page.host_str().unwrap_or("127.0.0.1");
    let secure = page.scheme() == "https";
    let scheme = if secure { "wss" } else { "ws" };
    let port = page.port().unwrap_or(if secure { 443 } else { 8443 });
    format!("{scheme}://{host}:{port}/ws")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_printed_invite() {
        let url = "http://203.0.113.10:8443/?s=friends-night&p=482193&auto=1&ws=ws://203.0.113.10:8443/ws&turn=turn:203.0.113.10:3478&turnu=cluser&turnp=secret";
        let p = parse_join_url(url).unwrap();
        assert_eq!(p.session_id, "friends-night");
        assert_eq!(p.pin, "482193");
        assert_eq!(p.signaling, "ws://203.0.113.10:8443/ws");
        assert_eq!(p.turn_url.as_deref(), Some("turn:203.0.113.10:3478"));
        assert!(p.mesh.is_none());
        assert!(p.hs_url.is_none());
    }

    #[test]
    fn parses_tailscale_mesh_invite() {
        let url = "http://100.64.1.2:8443/?s=a&p=1&auto=1&ws=ws://100.64.1.2:8443/ws&mesh=tailscale";
        let p = parse_join_url(url).unwrap();
        assert_eq!(p.mesh.as_deref(), Some("tailscale"));
        assert!(is_tailscale_invite(&p));
        assert!(!is_headscale_invite(&p));
    }

    #[test]
    fn parses_headscale_mesh_invite() {
        let url = "http://100.64.1.3:8443/?s=a&p=1&auto=1&ws=ws://100.64.1.3:8443/ws&mesh=headscale&hs=https%3A%2F%2Fhs.example.com&tskey=tskey-auth-xyz";
        let p = parse_join_url(url).unwrap();
        assert_eq!(p.mesh.as_deref(), Some("headscale"));
        assert_eq!(p.hs_url.as_deref(), Some("https://hs.example.com"));
        assert_eq!(p.ts_authkey.as_deref(), Some("tskey-auth-xyz"));
        assert!(is_headscale_invite(&p));
        assert!(!is_tailscale_invite(&p));
    }

    #[test]
    fn infers_ws_from_page_origin() {
        let p = parse_join_url("https://game.example.com/?s=a&p=1").unwrap();
        assert_eq!(p.signaling, "wss://game.example.com:443/ws");
    }

    #[test]
    fn parse_input_accepts_url_or_session_pin() {
        let u = parse_join_input(
            "http://host:8443/?s=abc&p=123&ws=ws://host:8443/ws",
        )
        .unwrap();
        assert_eq!(u.session_id, "abc");
        let sp = parse_join_input("friends-night:482193").unwrap();
        assert_eq!(sp.session_id, "friends-night");
        assert_eq!(sp.pin, "482193");
        assert!(parse_join_input("only-session").is_err());
    }
}
