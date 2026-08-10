//! Print / encode join invites for the friend (Rohomieo invite methodology).

use url::Url;

pub struct TurnInfo<'a> {
    pub url: &'a str,
    pub user: &'a str,
    pub pass: &'a str,
}

pub struct HeadscaleInvite<'a> {
    pub server_url: &'a str,
    pub auth_key: &'a str,
}

pub fn player_invite_url(
    public_base: &str,
    session_id: &str,
    pin: &str,
    signaling_ws: &str,
    turn: Option<TurnInfo>,
    mesh: Option<&str>,
    headscale: Option<HeadscaleInvite<'_>>,
    wireguard_conf: Option<&str>,
) -> String {
    let mut base = public_base.trim_end_matches('/').to_string();
    if base.is_empty() {
        base = "http://127.0.0.1:8443".into();
    }
    let mut u = Url::parse(&format!("{base}/")).unwrap_or_else(|_| {
        Url::parse("http://127.0.0.1:8443/").expect("fallback")
    });
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("s", session_id)
            .append_pair("p", pin)
            .append_pair("auto", "1")
            .append_pair("ws", signaling_ws);
        if let Some(m) = mesh.filter(|s| !s.is_empty()) {
            q.append_pair("mesh", m);
        }
        if let Some(hs) = headscale {
            if !hs.server_url.is_empty() {
                q.append_pair("hs", hs.server_url.trim_end_matches('/'));
            }
            if !hs.auth_key.is_empty() {
                q.append_pair("tskey", hs.auth_key);
            }
        }
        // The whole WireGuard config rides in the link, so the friend imports
        // one URL instead of being sent a .conf out of band. Carrying the file
        // itself rather than its fields means there is no second schema to
        // drift out of sync with what setup-wireguard.sh writes.
        //
        // This is credential-bearing, exactly like `tskey` above — join URLs
        // are secrets and docs/HEADSCALE.md already says so.
        if let Some(conf) = wireguard_conf.filter(|c| !c.trim().is_empty()) {
            q.append_pair("wg", conf);
        }
        if let Some(t) = turn {
            q.append_pair("turn", t.url)
                .append_pair("turnu", t.user)
                .append_pair("turnp", t.pass);
        }
    }
    u.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_includes_mesh_tailscale() {
        let u = player_invite_url(
            "http://100.64.0.1:8443",
            "abc",
            "123456",
            "ws://100.64.0.1:8443/ws",
            None,
            Some("tailscale"),
            None,
            None,
        );
        assert!(u.contains("mesh=tailscale"));
        assert!(u.contains("s=abc"));
        assert!(!u.contains("turn="));
    }

    #[test]
    fn wireguard_conf_rides_in_the_link_and_survives_encoding() {
        // Newlines and `=` are what break naive query building; the whole point
        // is that the friend imports a link instead of receiving a file.
        let conf = "[Interface]\nAddress = 10.66.0.2/24\n\n[Peer]\nEndpoint = [2603::1]:51820\n";
        let u = player_invite_url(
            "http://h:8443",
            "abc",
            "123456",
            "ws://h:8443/ws",
            None,
            Some("wireguard"),
            None,
            Some(conf),
        );
        let parsed = Url::parse(&u).unwrap();
        let got = parsed
            .query_pairs()
            .find(|(k, _)| k == "wg")
            .map(|(_, v)| v.into_owned())
            .expect("wg param present");
        assert_eq!(got, conf);
    }

    #[test]
    fn blank_wireguard_conf_is_omitted_rather_than_sent_empty() {
        let u = player_invite_url(
            "http://h:8443",
            "abc",
            "123456",
            "ws://h:8443/ws",
            None,
            None,
            None,
            Some("   "),
        );
        assert!(!u.contains("wg="));
    }

    #[test]
    fn invite_includes_headscale_hs_and_tskey() {
        let u = player_invite_url(
            "http://100.64.0.2:8443",
            "abc",
            "123456",
            "ws://100.64.0.2:8443/ws",
            None,
            Some("headscale"),
            Some(HeadscaleInvite {
                server_url: "https://hs.example.com",
                auth_key: "hskey-auth-test",
            }),
            None,
        );
        assert!(u.contains("mesh=headscale"));
        assert!(u.contains("hs=https%3A%2F%2Fhs.example.com") || u.contains("hs=https://hs.example.com"));
        assert!(u.contains("tskey=hskey-auth-test"));
    }
}
