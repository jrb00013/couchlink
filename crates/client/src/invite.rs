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

    Ok(ParsedInvite {
        signaling,
        session_id,
        pin,
        turn_url,
        turn_user,
        turn_pass,
    })
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
    }

    #[test]
    fn infers_ws_from_page_origin() {
        let p = parse_join_url("https://game.example.com/?s=a&p=1").unwrap();
        assert_eq!(p.signaling, "wss://game.example.com:443/ws");
    }
}
