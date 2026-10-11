//! Kaleidoscope seats — host-side master enable + authoritative config broadcast.
//!
//! Encode path is untouched. This only tracks intent and fans `SplitScreenConfig`
//! over signaling. Control panel / scripts flip master via localhost HTTP or env.

use couchlink_proto::{NormRect, SignalMessage};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitSeatState {
    pub master_enabled: bool,
    pub host_view_mode: String,
    pub layout: String,
    pub regions: Vec<NormRect>,
    pub seat_region: Vec<u8>,
    pub rescan: bool,
}

impl Default for SplitSeatState {
    fn default() -> Self {
        let master = std::env::var("COUCHLINK_SPLIT_SEAT_MASTER")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        Self {
            master_enabled: master,
            host_view_mode: "full".into(),
            layout: "none".into(),
            regions: Vec::new(),
            seat_region: vec![0, 1, 2, 3],
            rescan: false,
        }
    }
}

impl SplitSeatState {
    pub fn to_config_msg(&self) -> SignalMessage {
        SignalMessage::SplitScreenConfig {
            master_enabled: self.master_enabled,
            host_view_mode: self.host_view_mode.clone(),
            layout: self.layout.clone(),
            regions: self.regions.clone(),
            seat_region: self.seat_region.clone(),
            rescan: self.rescan,
        }
    }
}

/// Apply a player's `SplitScreenSet` into host state (regions/layout from detector).
pub fn apply_player_set(state: &mut SplitSeatState, msg: &SignalMessage) {
    let SignalMessage::SplitScreenSet {
        layout,
        regions,
        rescan,
        ..
    } = msg
    else {
        return;
    };
    if !layout.is_empty() && layout != "none" {
        state.layout = layout.clone();
    }
    if !regions.is_empty() {
        state.regions = regions.clone();
        let n = state.regions.len().min(4) as u8;
        state.seat_region = (0..n).collect();
    }
    if *rescan {
        state.rescan = true;
    }
}

/// Localhost control plane for the Host Control Panel (never binds public interfaces).
pub async fn serve_control(
    state: Arc<Mutex<SplitSeatState>>,
    signal_out: mpsc::UnboundedSender<SignalMessage>,
) {
    let bind = std::env::var("COUCHLINK_SPLIT_SEAT_CTRL")
        .unwrap_or_else(|_| "127.0.0.1:18765".into());
    let listener = match TcpListener::bind(&bind).await {
        Ok(l) => {
            info!("split-seat control listening on http://{bind}");
            l
        }
        Err(e) => {
            warn!("split-seat control bind {bind}: {e}");
            return;
        }
    };
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            continue;
        };
        let state = Arc::clone(&state);
        let signal_out = signal_out.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let n = match sock.read(&mut buf).await {
                Ok(n) => n,
                Err(_) => return,
            };
            let req = String::from_utf8_lossy(&buf[..n]);
            let (status, body) = handle_http(&req, &state, &signal_out).await;
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
    }
}

async fn handle_http(
    req: &str,
    state: &Arc<Mutex<SplitSeatState>>,
    signal_out: &mpsc::UnboundedSender<SignalMessage>,
) -> (&'static str, String) {
    if req.starts_with("OPTIONS ") {
        return ("204 No Content", String::new());
    }
    if req.starts_with("GET /health") || req.starts_with("GET / ") {
        return ("200 OK", r#"{"ok":true,"service":"split-seat"}"#.into());
    }
    if req.starts_with("GET /v1/split-seat") {
        let s = state.lock().await;
        return (
            "200 OK",
            serde_json::to_string(&*s).unwrap_or_else(|_| "{}".into()),
        );
    }
    if req.starts_with("POST /v1/split-seat") {
        let body = req.split("\r\n\r\n").nth(1).unwrap_or("");
        let Ok(patch) = serde_json::from_str::<serde_json::Value>(body) else {
            return ("400 Bad Request", r#"{"error":"bad json"}"#.into());
        };
        let mut s = state.lock().await;
        if let Some(v) = patch.get("master_enabled").and_then(|x| x.as_bool()) {
            s.master_enabled = v;
        }
        if let Some(v) = patch.get("host_view_mode").and_then(|x| x.as_str()) {
            s.host_view_mode = v.to_string();
        }
        if let Some(v) = patch.get("layout").and_then(|x| x.as_str()) {
            s.layout = v.to_string();
        }
        if let Some(v) = patch.get("rescan").and_then(|x| x.as_bool()) {
            s.rescan = v;
        }
        if let Some(regions) = patch.get("regions").and_then(|x| x.as_array()) {
            s.regions = regions
                .iter()
                .filter_map(|r| serde_json::from_value::<NormRect>(r.clone()).ok())
                .collect();
        }
        let msg = s.to_config_msg();
        s.rescan = false;
        drop(s);
        let _ = signal_out.send(msg);
        let s = state.lock().await;
        return (
            "200 OK",
            serde_json::to_string(&*s).unwrap_or_else(|_| "{}".into()),
        );
    }
    ("404 Not Found", r#"{"error":"not found"}"#.into())
}

pub async fn broadcast_config(
    state: &Arc<Mutex<SplitSeatState>>,
    signal_out: &mpsc::UnboundedSender<SignalMessage>,
) {
    let mut s = state.lock().await;
    let msg = s.to_config_msg();
    s.rescan = false;
    let _ = signal_out.send(msg);
}
