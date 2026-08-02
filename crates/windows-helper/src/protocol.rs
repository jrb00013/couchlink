//! JSON-lines protocol for the Couchlink Windows helper pipe.

use serde::{Deserialize, Serialize};

fn default_signaling() -> u16 {
    8443
}

fn default_turn() -> u16 {
    3478
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    OnlinePrep {
        #[serde(default)]
        skip_map: bool,
        #[serde(default)]
        wsl_ip: Option<String>,
        #[serde(default = "default_signaling")]
        signaling_port: u16,
        #[serde(default = "default_turn")]
        turn_port: u16,
    },
    FirewallUnblock,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ping_ok(version: impl Into<String>) -> Self {
        Self {
            ok: true,
            op: Some("ping".into()),
            version: Some(version.into()),
            exit: None,
            error: None,
        }
    }

    pub fn ok_exit(op: &str, exit: i32) -> Self {
        Self {
            ok: exit == 0 || exit == 2,
            op: Some(op.into()),
            version: None,
            exit: Some(exit),
            error: if exit == 0 || exit == 2 {
                None
            } else {
                Some(format!("exit {exit}"))
            },
        }
    }

    pub fn err(op: Option<&str>, error: impl Into<String>) -> Self {
        Self {
            ok: false,
            op: op.map(str::to_string),
            version: None,
            exit: None,
            error: Some(error.into()),
        }
    }
}

pub fn parse_request_line(line: &str) -> Result<Request, serde_json::Error> {
    serde_json::from_str(line.trim())
}

pub fn response_line(resp: &Response) -> String {
    let mut s = serde_json::to_string(resp).unwrap_or_else(|_| {
        r#"{"ok":false,"error":"serialize failed"}"#.to_string()
    });
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_roundtrip() {
        let r: Request = serde_json::from_str(r#"{"op":"ping"}"#).unwrap();
        assert!(matches!(r, Request::Ping));
        let line = response_line(&Response::ping_ok("0.1.1"));
        let back: Response = serde_json::from_str(line.trim()).unwrap();
        assert!(back.ok);
        assert_eq!(back.op.as_deref(), Some("ping"));
    }

    #[test]
    fn online_prep_defaults() {
        let r: Request =
            serde_json::from_str(r#"{"op":"online_prep","skip_map":true}"#).unwrap();
        match r {
            Request::OnlinePrep {
                skip_map,
                signaling_port,
                turn_port,
                wsl_ip,
            } => {
                assert!(skip_map);
                assert_eq!(signaling_port, 8443);
                assert_eq!(turn_port, 3478);
                assert!(wsl_ip.is_none());
            }
            _ => panic!("expected OnlinePrep"),
        }
    }

    #[test]
    fn reject_unknown_op() {
        assert!(serde_json::from_str::<Request>(r#"{"op":"rm_rf"}"#).is_err());
    }

    #[test]
    fn firewall_unblock_tag() {
        let r: Request = serde_json::from_str(r#"{"op":"firewall_unblock"}"#).unwrap();
        assert!(matches!(r, Request::FirewallUnblock));
    }
}
