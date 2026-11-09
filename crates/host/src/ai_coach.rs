//! Opt-in AI game coach bridge — host side (see docs/AI_GAME_COACH.md).
//!
//! Off unless `COUCHLINK_AI_COACH=1`. When off, [`spawn`] returns `None`: no task,
//! no timer, no socket. When on, one slow background task (every few seconds,
//! never in a frame or input loop) pushes the current game window title and the
//! occupied seats' pad kinds to the coach container on loopback. Nothing flows
//! back into the host; the coach never touches capture, encode or the pad path.
//!
//! Sent: title string, seat slot numbers, pad kind names, bound seat numbers.
//! Never sent: session id, PIN, join link, signaling URL, ICE/TURN settings.

use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{info, warn};

const DEFAULT_COACH_ADDR: &str = "127.0.0.1:18790";
const TICK: Duration = Duration::from_secs(10);
/// Re-send an unchanged context this often so a restarted coach catches up.
const HEARTBEAT: Duration = Duration::from_secs(60);
const IO_TIMEOUT: Duration = Duration::from_secs(2);
const TITLE_MAX: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub struct SeatInfo {
    pub slot: u8,
    pub pad_kind: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CoachCfg {
    pub enabled: bool,
    pub addr: String,
    pub bound_seats: Vec<u8>,
    /// Static title override (tests, or emulators whose title the host cannot see).
    pub title_env: Option<String>,
    /// File holding the current window title (e.g. written by a capture-side helper).
    pub title_file: Option<String>,
}

fn truthy(v: &str) -> bool {
    matches!(v.trim(), "1" | "true" | "yes" | "on")
}

impl CoachCfg {
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let nonempty = |k: &str| get(k).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let bound_seats = parse_seats(&nonempty("COUCHLINK_AI_COACH_SEAT").unwrap_or_else(|| "1".into()));
        Self {
            enabled: get("COUCHLINK_AI_COACH").map(|v| truthy(&v)).unwrap_or(false),
            addr: nonempty("COUCHLINK_AI_COACH_ADDR").unwrap_or_else(|| DEFAULT_COACH_ADDR.into()),
            bound_seats,
            title_env: nonempty("COUCHLINK_AI_COACH_TITLE"),
            title_file: nonempty("COUCHLINK_AI_COACH_TITLE_FILE"),
        }
    }
}

/// "1,2" -> [1,2]; anything outside 0..=3 is dropped. Seat 0 is the host's own pad.
pub fn parse_seats(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s
        .split(',')
        .filter_map(|x| x.trim().parse::<u8>().ok())
        .filter(|n| *n <= 3)
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The coach context holds seat/title data, so it only ever goes to this machine.
pub fn loopback_addr(addr: &str) -> Option<SocketAddr> {
    addr.parse::<SocketAddr>().ok().filter(|a| a.ip().is_loopback())
}

pub fn context_json(title: &str, seats: &[SeatInfo], bound: &[u8]) -> String {
    let title: String = title.chars().filter(|c| !c.is_control()).take(TITLE_MAX).collect();
    json!({
        "title": title,
        "seats": seats.iter().map(|s| json!({"slot": s.slot, "pad_kind": s.pad_kind.clone().unwrap_or_default()})).collect::<Vec<_>>(),
        "bound_seats": bound,
    })
    .to_string()
}

async fn current_title(cfg: &CoachCfg) -> String {
    if let Some(t) = &cfg.title_env {
        return t.clone();
    }
    if let Some(p) = &cfg.title_file {
        if let Ok(mut f) = tokio::fs::File::open(p).await {
            let mut buf = vec![0u8; 512];
            if let Ok(n) = f.read(&mut buf).await {
                return String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").trim().to_string();
            }
        }
        return String::new();
    }
    // A pinned capture window (`COUCHLINK_CAPTURE_WINDOW`) is a title needle, good enough
    // when the emulator already shows the game name.
    std::env::var("COUCHLINK_CAPTURE_WINDOW").unwrap_or_default()
}

async fn post_context(addr: SocketAddr, body: &str) -> std::io::Result<()> {
    let work = async {
        let mut s = TcpStream::connect(addr).await?;
        let req = format!(
            "POST /v1/context HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await?;
        let mut sink = [0u8; 256];
        let _ = s.read(&mut sink).await?;
        Ok::<_, std::io::Error>(())
    };
    tokio::time::timeout(IO_TIMEOUT, work)
        .await
        .unwrap_or_else(|_| Err(std::io::ErrorKind::TimedOut.into()))
}

/// Start the bridge if enabled. `seats` must be a cheap, non-blocking snapshot
/// (the caller uses `try_lock`; a contended tick is simply skipped).
pub fn spawn<F>(cfg: CoachCfg, seats: F) -> Option<tokio::task::JoinHandle<()>>
where
    F: Fn() -> Option<Vec<SeatInfo>> + Send + 'static,
{
    if !cfg.enabled {
        return None;
    }
    let Some(addr) = loopback_addr(&cfg.addr) else {
        warn!("ai-coach: COUCHLINK_AI_COACH_ADDR={} is not a loopback ip:port — coach bridge disabled", cfg.addr);
        return None;
    };
    info!("ai-coach: bridge on -> {addr}, bound seats {:?}", cfg.bound_seats);
    Some(tokio::spawn(async move {
        let mut last = String::new();
        let mut last_sent = tokio::time::Instant::now() - HEARTBEAT;
        let mut warned = false;
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(seat_list) = seats() else { continue };
            let body = context_json(&current_title(&cfg).await, &seat_list, &cfg.bound_seats);
            if body == last && last_sent.elapsed() < HEARTBEAT {
                continue;
            }
            match post_context(addr, &body).await {
                Ok(()) => {
                    last = body;
                    last_sent = tokio::time::Instant::now();
                    warned = false;
                }
                // The coach container is optional and may simply not be up: warn once, keep quiet after.
                Err(e) if !warned => {
                    warn!("ai-coach: coach not reachable at {addr}: {e} (run scripts/ai-coach.sh up)");
                    warned = true;
                }
                Err(_) => {}
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> CoachCfg {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        CoachCfg::from_lookup(|k| m.get(k).cloned())
    }

    #[test]
    fn off_by_default_and_spawn_is_inert() {
        let c = cfg(&[]);
        assert!(!c.enabled);
        assert!(spawn(c, || None).is_none());
        for v in ["0", "", "false", "off", "no"] {
            assert!(!cfg(&[("COUCHLINK_AI_COACH", v)]).enabled, "{v}");
        }
        assert!(cfg(&[("COUCHLINK_AI_COACH", "1")]).enabled);
    }

    #[test]
    fn defaults_bind_seat_one_loopback() {
        let c = cfg(&[("COUCHLINK_AI_COACH", "true")]);
        assert_eq!(c.bound_seats, vec![1]);
        assert_eq!(c.addr, DEFAULT_COACH_ADDR);
    }

    #[test]
    fn seat_parsing() {
        assert_eq!(parse_seats("2, 1,1,9,x,3"), vec![1, 2, 3]);
        assert!(parse_seats("").is_empty());
        assert!(parse_seats("4,255,-1").is_empty());
    }

    #[test]
    fn only_loopback_targets() {
        assert!(loopback_addr("127.0.0.1:18790").is_some());
        assert!(loopback_addr("[::1]:18790").is_some());
        for a in ["192.168.1.5:18790", "0.0.0.0:18790", "example.com:80", "8.8.8.8:80", "localhost:1"] {
            assert!(loopback_addr(a).is_none(), "{a}");
        }
        assert!(spawn(cfg(&[("COUCHLINK_AI_COACH", "1"), ("COUCHLINK_AI_COACH_ADDR", "10.0.0.2:1")]), || None).is_none());
    }

    #[test]
    fn context_json_carries_only_title_and_seats() {
        let seats = vec![
            SeatInfo { slot: 0, pad_kind: Some("dualsense".into()) },
            SeatInfo { slot: 1, pad_kind: None },
        ];
        let j = context_json("Mortal Kombat [BLUS30522]\n\x1b[31m", &seats, &[1]);
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["title"], "Mortal Kombat [BLUS30522][31m");
        assert_eq!(v["seats"][1]["pad_kind"], "");
        assert_eq!(v["bound_seats"], json!([1]));
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 3, "{keys:?}");
        assert!(context_json(&"x".repeat(999), &[], &[]).len() < 300);
    }

    #[tokio::test]
    async fn posts_context_to_a_loopback_listener() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let srv = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 2048];
            let n = s.read(&mut buf).await.unwrap();
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
            String::from_utf8_lossy(&buf[..n]).to_string()
        });
        post_context(addr, r#"{"title":"x"}"#).await.unwrap();
        let got = srv.await.unwrap();
        assert!(got.starts_with("POST /v1/context HTTP/1.1"));
        assert!(got.ends_with(r#"{"title":"x"}"#));
    }

    #[tokio::test]
    async fn unreachable_coach_errors_fast() {
        // Port 1 on loopback: refused, never hangs the (background) task.
        let r = post_context("127.0.0.1:1".parse().unwrap(), "{}").await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn title_sources_prefer_env_then_file() {
        let c = cfg(&[("COUCHLINK_AI_COACH_TITLE", "From Env")]);
        assert_eq!(current_title(&c).await, "From Env");
        let c = cfg(&[("COUCHLINK_AI_COACH_TITLE_FILE", "/nonexistent/couchlink-title")]);
        assert_eq!(current_title(&c).await, "");
    }
}
