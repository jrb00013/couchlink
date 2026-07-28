//! Print / encode join invites for the friend (Rohomieo invite methodology).

use url::Url;

pub struct TurnInfo<'a> {
    pub url: &'a str,
    pub user: &'a str,
    pub pass: &'a str,
}

pub fn player_invite_url(
    public_base: &str,
    session_id: &str,
    pin: &str,
    signaling_ws: &str,
    turn: Option<TurnInfo>,
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
        if let Some(t) = turn {
            q.append_pair("turn", t.url)
                .append_pair("turnu", t.user)
                .append_pair("turnp", t.pass);
        }
    }
    u.to_string()
}
