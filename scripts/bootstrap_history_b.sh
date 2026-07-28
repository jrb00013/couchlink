#!/usr/bin/env bash
# Part B — signaling, host, client, docs, scripts, ~50 commits total
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

commit() {
  local msg="$1"
  shift
  git add "$@"
  if git diff --cached --quiet 2>/dev/null; then
    return 0
  fi
  git commit -m "$msg"
}

w() {
  local path="$1"
  mkdir -p "$(dirname "$path")"
  cat > "$path"
}

############################################
# signaling crate
############################################
w crates/signaling/Cargo.toml <<'EOF'
[package]
name = "couchlink-signaling"
version.workspace = true
edition.workspace = true
description = "WebSocket signaling for couchlink co-play sessions"

[[bin]]
name = "couchlink-signaling"
path = "src/main.rs"

[dependencies]
couchlink-proto = { workspace = true }
anyhow = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
uuid = { workspace = true }
chrono = { workspace = true }
clap = { workspace = true }
futures-util = "0.3"
axum = { version = "0.8", features = ["ws", "macros"] }
axum-server = { version = "0.7", features = ["tls-rustls"] }
rustls = { version = "0.23", features = ["ring"] }
rustls-pemfile = "2"
tower-http = { version = "0.6", features = ["fs", "cors", "trace"] }
dashmap = "6"
EOF

w crates/signaling/src/audit.rs <<'EOF'
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventKind {
    HostRegistered,
    PlayerRegistered,
    PeerLeft,
    PinFailure,
    SessionExpired,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    pub ts: DateTime<Utc>,
    pub session_id: String,
    pub kind: AuditEventKind,
    pub detail: Option<String>,
}

pub struct AuditLog {
    events: Mutex<Vec<AuditEvent>>,
    cap: usize,
}

impl AuditLog {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            cap: 500,
        }
    }

    pub fn record(&self, session_id: &str, kind: AuditEventKind, detail: Option<String>) {
        let mut g = self.events.lock().unwrap();
        g.push(AuditEvent {
            ts: Utc::now(),
            session_id: session_id.into(),
            kind,
            detail,
        });
        if g.len() > self.cap {
            let drain = g.len() - self.cap;
            g.drain(0..drain);
        }
    }

    pub fn snapshot(&self) -> Vec<AuditEvent> {
        self.events.lock().unwrap().clone()
    }
}
EOF

w crates/signaling/src/metrics.rs <<'EOF'
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Metrics {
    pub sessions_active: AtomicU64,
    pub hosts_registered: AtomicU64,
    pub players_registered: AtomicU64,
    pub pin_failures: AtomicU64,
    pub ws_connections: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prometheus(&self) -> String {
        format!(
            "# TYPE couchlink_sessions_active gauge\ncouchlink_sessions_active {}\n\
             # TYPE couchlink_hosts_registered_total counter\ncouchlink_hosts_registered_total {}\n\
             # TYPE couchlink_players_registered_total counter\ncouchlink_players_registered_total {}\n\
             # TYPE couchlink_pin_failures_total counter\ncouchlink_pin_failures_total {}\n\
             # TYPE couchlink_ws_connections gauge\ncouchlink_ws_connections {}\n",
            self.sessions_active.load(Ordering::Relaxed),
            self.hosts_registered.load(Ordering::Relaxed),
            self.players_registered.load(Ordering::Relaxed),
            self.pin_failures.load(Ordering::Relaxed),
            self.ws_connections.load(Ordering::Relaxed),
        )
    }
}
EOF

commit "feat(signaling): audit log and prometheus metrics" \
  crates/signaling/Cargo.toml crates/signaling/src/audit.rs crates/signaling/src/metrics.rs

w crates/signaling/src/session.rs <<'EOF'
//! Session store — Rohomieo PIN + lockout methodology for host/player pairs.

use crate::audit::{AuditEventKind, AuditLog};
use crate::metrics::Metrics;
use chrono::{DateTime, Utc};
use couchlink_proto::Role;
use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub type WsSender = mpsc::UnboundedSender<String>;

pub struct PeerSlot {
    pub tx: Option<WsSender>,
}

pub struct Session {
    pub pin: String,
    pub device_name: Option<String>,
    pub preset: Option<String>,
    pub emulator: Option<String>,
    pub host: PeerSlot,
    pub player: PeerSlot,
    pub pin_failures: u32,
    pub locked_until: Option<DateTime<Utc>>,
    pub last_activity: DateTime<Utc>,
}

pub struct SessionStore {
    sessions: DashMap<String, Session>,
    connections: AtomicUsize,
    audit: Arc<AuditLog>,
    metrics: Arc<Metrics>,
    max_pin_failures: u32,
    session_ttl: Duration,
}

impl SessionStore {
    pub fn with_limits(
        audit: Arc<AuditLog>,
        metrics: Arc<Metrics>,
        max_pin_failures: u32,
        session_ttl_secs: u64,
    ) -> Self {
        Self {
            sessions: DashMap::new(),
            connections: AtomicUsize::new(0),
            audit,
            metrics,
            max_pin_failures,
            session_ttl: Duration::from_secs(session_ttl_secs),
        }
    }

    pub fn metrics(&self) -> Arc<Metrics> {
        Arc::clone(&self.metrics)
    }

    pub fn connection_count(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn inc_conn(&self) {
        self.connections.fetch_add(1, Ordering::Relaxed);
        self.metrics.ws_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn dec_conn(&self) {
        self.connections.fetch_sub(1, Ordering::Relaxed);
        self.metrics.ws_connections.fetch_sub(1, Ordering::Relaxed);
    }

    fn check_pin_lock(entry: &Session) -> Result<(), String> {
        if let Some(until) = entry.locked_until {
            if Utc::now() < until {
                return Err("too many failed PIN attempts — try again in a few minutes".into());
            }
        }
        Ok(())
    }

    fn record_pin_failure(&self, session_id: &str) {
        if let Some(mut entry) = self.sessions.get_mut(session_id) {
            entry.pin_failures += 1;
            self.metrics.pin_failures.fetch_add(1, Ordering::Relaxed);
            self.audit.record(
                session_id,
                AuditEventKind::PinFailure,
                Some(format!("attempt {}", entry.pin_failures)),
            );
            if entry.pin_failures >= self.max_pin_failures {
                entry.locked_until = Some(Utc::now() + chrono::Duration::minutes(5));
                entry.pin_failures = 0;
            }
        }
    }

    pub fn register_host(
        &self,
        session_id: String,
        pin: String,
        device_name: Option<String>,
        preset: Option<String>,
        emulator: Option<String>,
        tx: WsSender,
    ) -> Result<(), String> {
        let mut entry = self.sessions.entry(session_id.clone()).or_insert_with(|| {
            self.metrics.sessions_active.fetch_add(1, Ordering::Relaxed);
            Session {
                pin: pin.clone(),
                device_name: device_name.clone(),
                preset: preset.clone(),
                emulator: emulator.clone(),
                host: PeerSlot { tx: None },
                player: PeerSlot { tx: None },
                pin_failures: 0,
                locked_until: None,
                last_activity: Utc::now(),
            }
        });
        Self::check_pin_lock(&entry)?;
        if entry.pin != pin {
            drop(entry);
            self.record_pin_failure(&session_id);
            return Err("invalid PIN for session".into());
        }
        entry.device_name = device_name.or_else(|| entry.device_name.clone());
        entry.preset = preset.or_else(|| entry.preset.clone());
        entry.emulator = emulator.or_else(|| entry.emulator.clone());
        entry.host.tx = Some(tx);
        entry.last_activity = Utc::now();
        self.metrics.hosts_registered.fetch_add(1, Ordering::Relaxed);
        self.audit
            .record(&session_id, AuditEventKind::HostRegistered, None);
        Ok(())
    }

    pub fn register_player(
        &self,
        session_id: String,
        pin: String,
        tx: WsSender,
    ) -> Result<(), String> {
        let Some(mut entry) = self.sessions.get_mut(&session_id) else {
            return Err("unknown session".into());
        };
        Self::check_pin_lock(&entry)?;
        if entry.pin != pin {
            drop(entry);
            self.record_pin_failure(&session_id);
            return Err("invalid PIN for session".into());
        }
        entry.player.tx = Some(tx);
        entry.last_activity = Utc::now();
        self.metrics
            .players_registered
            .fetch_add(1, Ordering::Relaxed);
        self.audit
            .record(&session_id, AuditEventKind::PlayerRegistered, None);
        Ok(())
    }

    pub fn peer_tx(&self, session_id: &str, role: Role) -> Option<WsSender> {
        let entry = self.sessions.get(session_id)?;
        match role {
            Role::Host => entry.host.tx.clone(),
            Role::Player => entry.player.tx.clone(),
        }
    }

    pub fn relay(&self, session_id: &str, from: Role, msg: &str) {
        let to = match from {
            Role::Host => Role::Player,
            Role::Player => Role::Host,
        };
        if let Some(tx) = self.peer_tx(session_id, to) {
            let _ = tx.send(msg.to_string());
        }
        if let Some(mut s) = self.sessions.get_mut(session_id) {
            s.last_activity = Utc::now();
        }
    }

    pub fn unregister(&self, session_id: &str, role: Role) {
        let notify = {
            let Some(mut entry) = self.sessions.get_mut(session_id) else {
                return;
            };
            match role {
                Role::Host => entry.host.tx = None,
                Role::Player => entry.player.tx = None,
            }
            entry.last_activity = Utc::now();
            match role {
                Role::Host => entry.player.tx.clone(),
                Role::Player => entry.host.tx.clone(),
            }
        };
        if let Some(tx) = notify {
            let _ = tx.send(
                couchlink_proto::SignalMessage::PeerLeft
                    .to_json()
                    .unwrap_or_default(),
            );
        }
        self.audit
            .record(session_id, AuditEventKind::PeerLeft, Some(format!("{role:?}")));
    }

    pub fn sweep_expired(&self) {
        let now = Utc::now();
        let ttl = chrono::Duration::from_std(self.session_ttl).unwrap_or(chrono::Duration::hours(1));
        let stale: Vec<String> = self
            .sessions
            .iter()
            .filter(|e| now - e.last_activity > ttl)
            .map(|e| e.key().clone())
            .collect();
        for id in stale {
            self.sessions.remove(&id);
            self.metrics.sessions_active.fetch_sub(1, Ordering::Relaxed);
            self.audit
                .record(&id, AuditEventKind::SessionExpired, None);
        }
    }
}
EOF

commit "feat(signaling): session store with PIN lockout (Rohomieo method)" \
  crates/signaling/src/session.rs

w crates/signaling/src/ws.rs <<'EOF'
use crate::session::SessionStore;
use axum::extract::ws::{Message, WebSocket};
use couchlink_proto::{Role, SignalMessage};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;

pub async fn handle_socket(socket: WebSocket, store: Arc<SessionStore>) {
    store.inc_conn();
    let (sender, mut receiver) = socket.split();
    let sender = Arc::new(Mutex::new(sender));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let sender_fwd = Arc::clone(&sender);
    let forward = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let mut s = sender_fwd.lock().await;
            if s.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    let mut session_id: Option<String> = None;
    let mut role: Option<Role> = None;

    while let Some(Ok(msg)) = receiver.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Ping(p) => {
                let mut s = sender.lock().await;
                let _ = s.send(Message::Pong(p)).await;
                continue;
            }
            Message::Close(_) => break,
            _ => continue,
        };

        let parsed = match SignalMessage::from_json(&text) {
            Ok(m) => m,
            Err(e) => {
                let _ = tx.send(
                    SignalMessage::Error {
                        message: format!("invalid message: {e}"),
                    }
                    .to_json()
                    .unwrap(),
                );
                continue;
            }
        };

        match parsed {
            SignalMessage::RegisterHost {
                session_id: sid,
                pin,
                device_name,
                preset,
                emulator,
            } => {
                if let Err(e) = store.register_host(
                    sid.clone(),
                    pin,
                    device_name,
                    preset,
                    emulator,
                    tx.clone(),
                ) {
                    let _ = tx.send(
                        SignalMessage::Error { message: e }.to_json().unwrap(),
                    );
                    continue;
                }
                session_id = Some(sid.clone());
                role = Some(Role::Host);
                let _ = tx.send(
                    SignalMessage::Registered {
                        role: Role::Host,
                        session_id: sid,
                    }
                    .to_json()
                    .unwrap(),
                );
                debug!("host registered");
            }
            SignalMessage::RegisterPlayer {
                session_id: sid,
                pin,
                player_name: _,
            } => {
                if let Err(e) = store.register_player(sid.clone(), pin, tx.clone()) {
                    let _ = tx.send(
                        SignalMessage::Error { message: e }.to_json().unwrap(),
                    );
                    continue;
                }
                session_id = Some(sid.clone());
                role = Some(Role::Player);
                let _ = tx.send(
                    SignalMessage::Registered {
                        role: Role::Player,
                        session_id: sid.clone(),
                    }
                    .to_json()
                    .unwrap(),
                );
                if let Some(host_tx) = store.peer_tx(&sid, Role::Host) {
                    let _ = host_tx.send(
                        SignalMessage::PeerJoined {
                            role: Role::Player,
                        }
                        .to_json()
                        .unwrap(),
                    );
                }
                debug!("player registered");
            }
            SignalMessage::Heartbeat => {
                let _ = tx.send(SignalMessage::Pong.to_json().unwrap());
            }
            SignalMessage::Offer { .. }
            | SignalMessage::Answer { .. }
            | SignalMessage::IceCandidate { .. }
            | SignalMessage::StreamInfo { .. } => {
                if let (Some(sid), Some(r)) = (&session_id, role) {
                    store.relay(sid, r, &text);
                }
            }
            _ => {}
        }
    }

    if let (Some(sid), Some(r)) = (session_id, role) {
        store.unregister(&sid, r);
    }
    store.dec_conn();
    forward.abort();
}
EOF

commit "feat(signaling): WebSocket handler for host/player + SDP relay" \
  crates/signaling/src/ws.rs

w crates/signaling/src/api.rs <<'EOF'
use crate::session::SessionStore;
use axum::{extract::State, Json};
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone)]
pub struct ApiState {
    pub store: Arc<SessionStore>,
    pub audit: Arc<crate::audit::AuditLog>,
    pub version: &'static str,
}

#[derive(Serialize)]
pub struct Status {
    pub status: &'static str,
    pub version: &'static str,
    pub ws_connections: usize,
    pub sessions_active: usize,
}

pub async fn health() -> &'static str {
    "ok"
}

pub async fn api_status(State(st): State<ApiState>) -> Json<Status> {
    Json(Status {
        status: "ok",
        version: st.version,
        ws_connections: st.store.connection_count(),
        sessions_active: st.store.session_count(),
    })
}

pub async fn api_audit(State(st): State<ApiState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(st.audit.snapshot()).unwrap_or_default())
}

pub async fn metrics_handler(State(st): State<ApiState>) -> String {
    st.store.metrics().prometheus()
}
EOF

w crates/signaling/src/main.rs <<'EOF'
mod api;
mod audit;
mod metrics;
mod session;
mod ws;

use anyhow::Context;
use api::{api_audit, api_status, health, metrics_handler, ApiState};
use audit::AuditLog;
use axum::{
    extract::{ws::WebSocketUpgrade, State},
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::Parser;
use metrics::Metrics;
use session::SessionStore;
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "couchlink-signaling", about = "Couchlink signaling server", version)]
struct Args {
    #[arg(long, default_value = "0.0.0.0:8443", env = "COUCHLINK_BIND")]
    bind: SocketAddr,
    #[arg(long, default_value = "../../web/dist")]
    web_root: PathBuf,
    #[arg(long, env = "COUCHLINK_CERT")]
    cert: Option<PathBuf>,
    #[arg(long, env = "COUCHLINK_KEY")]
    key: Option<PathBuf>,
    #[arg(long, default_value = "3600", env = "COUCHLINK_SESSION_TTL_SECS")]
    session_ttl_secs: u64,
    #[arg(long, default_value = "5", env = "COUCHLINK_MAX_PIN_FAILURES")]
    max_pin_failures: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "couchlink_signaling=info,tower_http=info".into()),
        )
        .init();

    let args = Args::parse();
    let audit = Arc::new(AuditLog::new());
    let metrics = Arc::new(Metrics::new());
    let store = Arc::new(SessionStore::with_limits(
        Arc::clone(&audit),
        Arc::clone(&metrics),
        args.max_pin_failures,
        args.session_ttl_secs,
    ));

    let api_state = ApiState {
        store: Arc::clone(&store),
        audit: Arc::clone(&audit),
        version: env!("CARGO_PKG_VERSION"),
    };

    let sweep = Arc::clone(&store);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            sweep.sweep_expired();
        }
    });

    let web_root = args
        .web_root
        .canonicalize()
        .unwrap_or(args.web_root.clone());
    let index = web_root.join("index.html");
    let serve_dir = ServeDir::new(&web_root).not_found_service(ServeFile::new(index));

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/status", get(api_status))
        .route("/api/audit", get(api_audit))
        .route("/metrics", get(metrics_handler))
        .route(
            "/ws",
            get({
                let store = Arc::clone(&store);
                move |ws: WebSocketUpgrade| {
                    let store = Arc::clone(&store);
                    async move { ws.on_upgrade(move |s| ws::handle_socket(s, store)) }
                }
            }),
        )
        .with_state(api_state)
        .fallback_service(serve_dir)
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .layer(TraceLayer::new_for_http());

    info!("couchlink signaling on {}", args.bind);

    if let (Some(cert), Some(key)) = (args.cert, args.key) {
        let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
            .await
            .context("load TLS")?;
        axum_server::bind_rustls(args.bind, config)
            .serve(app.into_make_service())
            .await?;
    } else {
        let listener = tokio::net::TcpListener::bind(args.bind).await?;
        axum::serve(listener, app).await?;
    }
    Ok(())
}
EOF

commit "feat(signaling): HTTP health/status/metrics + TLS-capable server" \
  crates/signaling/src/api.rs crates/signaling/src/main.rs

############################################
# host crate
############################################
w crates/host/Cargo.toml <<'EOF'
[package]
name = "couchlink-host"
version.workspace = true
edition.workspace = true
description = "Couchlink host — HD capture, WebRTC stream, virtual BT pad inject"

[[bin]]
name = "couchlink-host"
path = "src/main.rs"

[dependencies]
couchlink-proto = { workspace = true }
couchlink-pad = { workspace = true }
anyhow = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
uuid = { workspace = true }
bytes = { workspace = true }
clap = { workspace = true }
futures-util = "0.3"
webrtc = "0.12"
interceptor = "0.12"
tokio-tungstenite = { version = "0.24", default-features = false, features = ["connect", "rustls-tls-webpki-roots"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
url = "2"
scrap = "0.5"
openh264 = "0.6"
rand = "0.8"
EOF

w crates/host/src/config.rs <<'EOF'
use clap::Parser;
use couchlink_proto::StreamPreset;

#[derive(Parser, Debug, Clone)]
#[command(name = "couchlink-host", about = "Host co-play session for emulators", version)]
pub struct HostArgs {
    #[arg(long, env = "COUCHLINK_SIGNALING", default_value = "ws://127.0.0.1:8443/ws")]
    pub signaling: String,
    #[arg(long, env = "COUCHLINK_SESSION_ID")]
    pub session_id: String,
    #[arg(long, env = "COUCHLINK_PIN")]
    pub pin: String,
    #[arg(long, default_value = "couchlink-host")]
    pub device_name: String,
    #[arg(long, default_value = "1080p60", env = "COUCHLINK_PRESET")]
    pub preset: String,
    #[arg(long, default_value = "auto")]
    pub emulator: String,
    /// Idle FPS when motion detector sees a still frame (Rohomieo method).
    #[arg(long, default_value = "8")]
    pub idle_fps: u32,
    #[arg(long, default_value_t = true)]
    pub bluetooth_pad: bool,
}

impl HostArgs {
    pub fn stream_preset(&self) -> StreamPreset {
        StreamPreset::parse(&self.preset).unwrap_or(StreamPreset::P1080_60)
    }
}
EOF

commit "feat(host): CLI config with HD presets and idle FPS" \
  crates/host/Cargo.toml crates/host/src/config.rs

w crates/host/src/motion.rs <<'EOF'
//! Tile-diff motion detector — Rohomieo methodology.
//! Skips encode / drops to idle FPS when <2% of sampled tiles change.

pub struct MotionDetector {
    prev: Vec<u8>,
    width: u32,
    height: u32,
    tile: u32,
}

impl MotionDetector {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            prev: Vec::new(),
            width,
            height,
            tile: 32,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        self.prev.clear();
    }

    /// Returns fraction of changed tiles in 0.0..=1.0 (BGRA input).
    pub fn changed_fraction(&mut self, bgra: &[u8]) -> f32 {
        let stride = (self.width as usize) * 4;
        if bgra.len() < stride * self.height as usize {
            return 1.0;
        }
        let tw = self.tile.max(8);
        let tiles_x = (self.width / tw).max(1);
        let tiles_y = (self.height / tw).max(1);
        let need = (tiles_x * tiles_y) as usize;
        if self.prev.len() != need {
            self.prev = vec![0u8; need];
        }
        let mut changed = 0u32;
        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let x = (tx * tw + tw / 2) as usize;
                let y = (ty * tw + tw / 2) as usize;
                let i = y * stride + x * 4;
                let sample = bgra[i] ^ bgra[i + 1] ^ bgra[i + 2];
                let idx = (ty * tiles_x + tx) as usize;
                if self.prev[idx] != sample {
                    changed += 1;
                    self.prev[idx] = sample;
                }
            }
        }
        changed as f32 / need as f32
    }

    pub fn is_idle(&mut self, bgra: &[u8]) -> bool {
        self.changed_fraction(bgra) < 0.02
    }
}
EOF

commit "feat(host): Rohomieo-style tile motion detector for idle FPS" \
  crates/host/src/motion.rs

w crates/host/src/capture.rs <<'EOF'
//! Screen / window capture via scrap (DXGI / X11 / Quartz) — same stack as Rohomieo.

use anyhow::{Context, Result};
use scrap::{Capturer, Display};

pub struct FrameCapture {
    capturer: Capturer,
    pub width: usize,
    pub height: usize,
}

impl FrameCapture {
    pub fn primary() -> Result<Self> {
        let display = Display::primary().context("no primary display")?;
        let width = display.width();
        let height = display.height();
        let capturer = Capturer::new(display).context("create capturer")?;
        Ok(Self {
            capturer,
            width,
            height,
        })
    }

    pub fn capture_bgra(&mut self) -> Result<Option<Vec<u8>>> {
        match self.capturer.frame() {
            Ok(frame) => Ok(Some(frame.to_vec())),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
EOF

w crates/host/src/encode.rs <<'EOF'
//! OpenH264 encode path targeting HD low-latency (baseline, low latency tune).

use anyhow::{Context, Result};
use openh264::encoder::{Encoder, EncoderConfig};
use openh264::formats::YUVBuffer;
use openh264::OpenH264API;

pub struct H264Encoder {
    enc: Encoder,
    pub width: u32,
    pub height: u32,
    pub bitrate_kbps: u32,
}

impl H264Encoder {
    pub fn new(width: u32, height: u32, bitrate_kbps: u32) -> Result<Self> {
        let api = OpenH264API::from_source();
        let cfg = EncoderConfig::new(width, height)
            .max_frame_rate(60.0)
            .bitrate_bps((bitrate_kbps * 1000).max(1_000_000));
        let enc = Encoder::with_api_config(api, cfg).context("openh264 encoder")?;
        Ok(Self {
            enc,
            width,
            height,
            bitrate_kbps,
        })
    }

    /// Encode BGRA frame → Annex-B access unit bytes.
    pub fn encode_bgra(&mut self, bgra: &[u8]) -> Result<Option<Vec<u8>>> {
        let w = self.width as usize;
        let h = self.height as usize;
        if bgra.len() < w * h * 4 {
            return Ok(None);
        }
        // Convert BGRA → RGB for YUVBuffer helper
        let mut rgb = vec![0u8; w * h * 3];
        for i in 0..(w * h) {
            rgb[i * 3] = bgra[i * 4 + 2];
            rgb[i * 3 + 1] = bgra[i * 4 + 1];
            rgb[i * 3 + 2] = bgra[i * 4];
        }
        let yuv = YUVBuffer::from_rgb8(w, h, &rgb);
        let bitstream = self.enc.encode(&yuv).context("encode")?;
        let mut out = Vec::new();
        bitstream.write_vec(&mut out);
        if out.is_empty() {
            Ok(None)
        } else {
            Ok(Some(out))
        }
    }
}
EOF

commit "feat(host): scrap capture + OpenH264 HD encode path" \
  crates/host/src/capture.rs crates/host/src/encode.rs

w crates/host/src/signaling_client.rs <<'EOF'
use anyhow::{bail, Context, Result};
use couchlink_proto::SignalMessage;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

pub struct SignalingClient {
    pub outbound: mpsc::UnboundedSender<SignalMessage>,
    pub inbound: mpsc::UnboundedReceiver<SignalMessage>,
}

impl SignalingClient {
    pub async fn connect(url: &str) -> Result<Self> {
        let (ws, _) = connect_async(url)
            .await
            .with_context(|| format!("connect signaling {url}"))?;
        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<SignalMessage>();
        let (in_tx, in_rx) = mpsc::unbounded_channel::<SignalMessage>();

        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                match msg.to_json() {
                    Ok(j) => {
                        if sink.send(Message::Text(j.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => warn!("signal encode: {e}"),
                }
            }
        });

        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Text(t) = msg else { continue };
                match SignalMessage::from_json(&t) {
                    Ok(m) => {
                        if in_tx.send(m).is_err() {
                            break;
                        }
                    }
                    Err(e) => warn!("signal decode: {e}"),
                }
            }
        });

        info!("signaling connected");
        Ok(Self {
            outbound: out_tx,
            inbound: in_rx,
        })
    }

    pub async fn register_host(
        &mut self,
        session_id: String,
        pin: String,
        device_name: String,
        preset: String,
        emulator: String,
    ) -> Result<()> {
        self.outbound.send(SignalMessage::RegisterHost {
            session_id: session_id.clone(),
            pin,
            device_name: Some(device_name),
            preset: Some(preset),
            emulator: Some(emulator),
        })?;
        while let Some(msg) = self.inbound.recv().await {
            match msg {
                SignalMessage::Registered { role, .. } => {
                    info!("registered as {role:?}");
                    return Ok(());
                }
                SignalMessage::Error { message } => bail!("signaling: {message}"),
                _ => {}
            }
        }
        bail!("signaling closed before register ack")
    }
}
EOF

commit "feat(host): WebSocket signaling client" \
  crates/host/src/signaling_client.rs

w crates/host/src/webrtc_peer.rs <<'EOF'
//! WebRTC host peer — video track + `pad` DataChannel (Rohomieo offer flow).

use anyhow::{Context, Result};
use bytes::BytesMut;
use couchlink_pad::{VirtualPad, VirtualPadConfig};
use couchlink_proto::{PadFeedback, PadFrame, SignalMessage, PAD_CHANNEL};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264};
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;
use webrtc::media::Sample;
use std::time::Duration;

pub struct WebRtcHost {
    pub pc: Arc<RTCPeerConnection>,
    pub video: Arc<TrackLocalStaticSample>,
    pub pad_tx: mpsc::UnboundedSender<PadFrame>,
}

impl WebRtcHost {
    pub async fn new(
        signal_out: mpsc::UnboundedSender<SignalMessage>,
        pad_device: Arc<Mutex<VirtualPad>>,
        as_bluetooth: bool,
    ) -> Result<(Self, mpsc::UnboundedReceiver<PadFrame>)> {
        let _ = as_bluetooth;
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();

        // Empty ICE servers → LAN / WireGuard only (Rohomieo security posture).
        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec![],
                ..Default::default()
            }],
            ..Default::default()
        };
        let pc = Arc::new(api.new_peer_connection(config).await?);

        let video = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_owned(),
                ..Default::default()
            },
            "video".to_owned(),
            "couchlink".to_owned(),
        ));
        pc.add_track(Arc::clone(&video) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        let (pad_tx, pad_rx) = mpsc::unbounded_channel::<PadFrame>();
        let pad_tx_dc = pad_tx.clone();
        let pad_device_dc = Arc::clone(&pad_device);

        let pc2 = Arc::clone(&pc);
        let signal_ice = signal_out.clone();
        pc.on_ice_candidate(Box::new(move |c| {
            let signal_ice = signal_ice.clone();
            Box::pin(async move {
                if let Some(c) = c {
                    let _ = signal_ice.send(SignalMessage::IceCandidate {
                        candidate: c.to_json().await.unwrap_or_default().candidate,
                        sdp_mid: c.sdp_mid.clone(),
                        sdp_mline_index: c.sdp_mline_index.map(|v| v as u16),
                    });
                }
            })
        }));

        // Create pad data channel (host→negotiated with offer)
        let dc = pc2.create_data_channel(PAD_CHANNEL, None).await?;
        setup_pad_channel(dc, pad_tx_dc, pad_device_dc).await;

        Ok((
            Self {
                pc,
                video,
                pad_tx,
            },
            pad_rx,
        ))
    }

    pub async fn create_and_send_offer(
        &self,
        signal_out: &mpsc::UnboundedSender<SignalMessage>,
    ) -> Result<()> {
        let offer = self.pc.create_offer(None).await?;
        self.pc.set_local_description(offer).await?;
        let local = self
            .pc
            .local_description()
            .await
            .context("local description")?;
        signal_out.send(SignalMessage::Offer { sdp: local.sdp })?;
        Ok(())
    }

    pub async fn handle_answer(&self, sdp: String) -> Result<()> {
        let answer = RTCSessionDescription::answer(sdp)?;
        self.pc.set_remote_description(answer).await?;
        Ok(())
    }

    pub async fn add_ice(&self, candidate: String, mid: Option<String>, mline: Option<u16>) -> Result<()> {
        use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
        let init = RTCIceCandidateInit {
            candidate,
            sdp_mid: mid,
            sdp_mline_index: mline,
            ..Default::default()
        };
        self.pc.add_ice_candidate(init).await?;
        Ok(())
    }

    pub async fn push_h264(&self, annex_b: Vec<u8>, duration: Duration) -> Result<()> {
        self.video
            .write_sample(&Sample {
                data: bytes::Bytes::from(annex_b),
                duration,
                ..Default::default()
            })
            .await?;
        Ok(())
    }
}

async fn setup_pad_channel(
    dc: Arc<RTCDataChannel>,
    pad_tx: mpsc::UnboundedSender<PadFrame>,
    pad_device: Arc<Mutex<VirtualPad>>,
) {
    dc.on_open(Box::new(move || {
        info!("pad datachannel open");
        Box::pin(async {})
    }));

    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let pad_tx = pad_tx.clone();
        let pad_device = Arc::clone(&pad_device);
        Box::pin(async move {
            if msg.is_string {
                // feedback JSON ignored on host inbound (player→host is binary pads)
                if let Ok(text) = std::str::from_utf8(&msg.data) {
                    if let Ok(_fb) = serde_json::from_str::<PadFeedback>(text) {
                        // player shouldn't send feedback; ignore
                    }
                }
                return;
            }
            match PadFrame::decode(&msg.data) {
                Ok(frame) => {
                    let _ = pad_tx.send(frame);
                    let mut guard = pad_device.lock().await;
                    if let Err(e) = guard.apply(&frame) {
                        warn!("virtual pad apply: {e}");
                    }
                }
                Err(e) => warn!("bad pad frame: {e}"),
            }
        })
    }));
}

pub fn create_virtual_pad(as_bluetooth: bool) -> Result<VirtualPad> {
    let mut cfg = VirtualPadConfig::default();
    cfg.as_bluetooth = as_bluetooth;
    VirtualPad::create(cfg)
}

/// Helper kept for tests / demos without WebRTC.
pub fn apply_pad_bytes(pad: &mut VirtualPad, data: &[u8]) -> Result<()> {
    let frame = PadFrame::decode(data)?;
    pad.apply(&frame)?;
    let _ = BytesMut::new();
    Ok(())
}
EOF

commit "feat(host): WebRTC peer with H.264 track and pad DataChannel" \
  crates/host/src/webrtc_peer.rs

w crates/host/src/main.rs <<'EOF'
mod capture;
mod config;
mod encode;
mod motion;
mod signaling_client;
mod webrtc_peer;

use anyhow::Result;
use clap::Parser;
use config::HostArgs;
use couchlink_proto::SignalMessage;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "couchlink_host=info".into()),
        )
        .init();

    let args = HostArgs::parse();
    let preset = args.stream_preset();
    info!(
        "couchlink host session={} preset={}x{}@{} bluetooth_pad={}",
        args.session_id, preset.width, preset.height, preset.fps, args.bluetooth_pad
    );

    let pad = Arc::new(Mutex::new(webrtc_peer::create_virtual_pad(
        args.bluetooth_pad,
    )?));

    let mut signaling = signaling_client::SignalingClient::connect(&args.signaling).await?;
    signaling
        .register_host(
            args.session_id.clone(),
            args.pin.clone(),
            args.device_name.clone(),
            args.preset.clone(),
            args.emulator.clone(),
        )
        .await?;

    let signal_out = signaling.outbound.clone();
    let (host, mut _pad_rx) =
        webrtc_peer::WebRtcHost::new(signal_out.clone(), Arc::clone(&pad), args.bluetooth_pad)
            .await?;

    // Wait for player, then offer
    loop {
        let Some(msg) = signaling.inbound.recv().await else {
            break;
        };
        match msg {
            SignalMessage::PeerJoined { .. } => {
                info!("player joined — sending offer");
                host.create_and_send_offer(&signal_out).await?;
                break;
            }
            SignalMessage::Error { message } => warn!("signal error: {message}"),
            _ => {}
        }
    }

    let mut capturer = capture::FrameCapture::primary()?;
    let mut encoder = encode::H264Encoder::new(preset.width, preset.height, preset.bitrate_kbps)?;
    let mut motion = motion::MotionDetector::new(preset.width, preset.height);
    let frame_dur = Duration::from_millis(1000 / preset.fps.max(1) as u64);
    let idle_dur = Duration::from_millis(1000 / args.idle_fps.max(1) as u64);

    let _ = signal_out.send(SignalMessage::StreamInfo {
        width: preset.width,
        height: preset.height,
        fps: preset.fps,
        codec: "H264".into(),
    });

    loop {
        tokio::select! {
            msg = signaling.inbound.recv() => {
                match msg {
                    Some(SignalMessage::Answer { sdp }) => {
                        host.handle_answer(sdp).await?;
                        info!("remote answer set");
                    }
                    Some(SignalMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index }) => {
                        let _ = host.add_ice(candidate, sdp_mid, sdp_mline_index).await;
                    }
                    Some(SignalMessage::PeerLeft) => {
                        warn!("player left");
                    }
                    Some(SignalMessage::Heartbeat) => {
                        let _ = signal_out.send(SignalMessage::Pong);
                    }
                    None => break,
                    _ => {}
                }
            }
            _ = tokio::time::sleep(frame_dur) => {
                let Some(bgra) = capturer.capture_bgra()? else { continue };
                // Note: production path should scale capturer buffer to preset size.
                let idle = motion.is_idle(&bgra);
                if idle {
                    tokio::time::sleep(idle_dur.saturating_sub(frame_dur / 4)).await;
                }
                if let Some(nal) = encoder.encode_bgra(&bgra)? {
                    if let Err(e) = host.push_h264(nal, frame_dur).await {
                        warn!("push h264: {e}");
                    }
                }
            }
        }
    }

    Ok(())
}
EOF

commit "feat(host): session loop — capture, encode, stream, inject pad" \
  crates/host/src/main.rs

############################################
# client crate
############################################
w crates/client/Cargo.toml <<'EOF'
[package]
name = "couchlink-client"
version.workspace = true
edition.workspace = true
description = "Couchlink player — DualSense capture + WebRTC viewer"

[[bin]]
name = "couchlink-client"
path = "src/main.rs"

[dependencies]
couchlink-proto = { workspace = true }
couchlink-pad = { workspace = true }
anyhow = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
bytes = { workspace = true }
clap = { workspace = true }
futures-util = "0.3"
webrtc = "0.12"
interceptor = "0.12"
tokio-tungstenite = { version = "0.24", default-features = false, features = ["connect", "rustls-tls-webpki-roots"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
hidapi = "2"
EOF

w crates/client/src/dualsense_reader.rs <<'EOF'
//! Read local DualSense via hidapi — dualsensekit enumeration methodology.

use anyhow::{bail, Context, Result};
use couchlink_pad::dualsense::{PID_DUALSENSE, PID_DUALSENSE_EDGE, SONY_VID};
use couchlink_pad::parse_input_report;
use couchlink_proto::PadFrame;
use hidapi::{HidApi, HidDevice};
use tracing::info;

pub struct DualSenseReader {
    device: HidDevice,
    seq: u32,
}

impl DualSenseReader {
    pub fn open_first() -> Result<Self> {
        let api = HidApi::new().context("hidapi init")?;
        let mut candidates: Vec<_> = api
            .device_list()
            .filter(|d| {
                d.vendor_id() == SONY_VID
                    && (d.product_id() == PID_DUALSENSE || d.product_id() == PID_DUALSENSE_EDGE)
            })
            .collect();
        if candidates.is_empty() {
            bail!("no DualSense found (pair it first — see dualsensekit playbook)");
        }
        // Prefer USB (interface >= 0) like dualsensekit Python wrapper
        candidates.sort_by_key(|d| if d.interface_number() >= 0 { 0 } else { 1 });
        let info = candidates[0];
        let device = info.open_device(&api).context("open DualSense")?;
        info!(
            "opened DualSense pid={:04x} interface={}",
            info.product_id(),
            info.interface_number()
        );
        Ok(Self { device, seq: 0 })
    }

    pub fn read_frame(&mut self) -> Result<Option<PadFrame>> {
        let mut buf = [0u8; 128];
        let n = match self.device.read_timeout(&mut buf, 4) {
            Ok(n) => n,
            Err(_) => return Ok(None),
        };
        if n == 0 {
            return Ok(None);
        }
        let mut frame = match parse_input_report(&buf[..n]) {
            Some(f) => f,
            None => return Ok(None),
        };
        self.seq = self.seq.wrapping_add(1);
        frame.seq = self.seq;
        Ok(Some(frame))
    }
}
EOF

commit "feat(client): DualSense hidapi reader (dualsensekit enumeration)" \
  crates/client/Cargo.toml crates/client/src/dualsense_reader.rs

w crates/client/src/signaling_client.rs <<'EOF'
use anyhow::{bail, Context, Result};
use couchlink_proto::SignalMessage;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

pub struct SignalingClient {
    pub outbound: mpsc::UnboundedSender<SignalMessage>,
    pub inbound: mpsc::UnboundedReceiver<SignalMessage>,
}

impl SignalingClient {
    pub async fn connect(url: &str) -> Result<Self> {
        let (ws, _) = connect_async(url)
            .await
            .with_context(|| format!("connect {url}"))?;
        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<SignalMessage>();
        let (in_tx, in_rx) = mpsc::unbounded_channel::<SignalMessage>();

        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                if let Ok(j) = msg.to_json() {
                    if sink.send(Message::Text(j.into())).await.is_err() {
                        break;
                    }
                }
            }
        });
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Text(t) = msg else { continue };
                match SignalMessage::from_json(&t) {
                    Ok(m) => {
                        if in_tx.send(m).is_err() {
                            break;
                        }
                    }
                    Err(e) => warn!("decode: {e}"),
                }
            }
        });
        Ok(Self {
            outbound: out_tx,
            inbound: in_rx,
        })
    }

    pub async fn register_player(&mut self, session_id: String, pin: String) -> Result<()> {
        self.outbound.send(SignalMessage::RegisterPlayer {
            session_id,
            pin,
            player_name: None,
        })?;
        while let Some(msg) = self.inbound.recv().await {
            match msg {
                SignalMessage::Registered { .. } => {
                    info!("player registered");
                    return Ok(());
                }
                SignalMessage::Error { message } => bail!("{message}"),
                _ => {}
            }
        }
        bail!("closed")
    }
}
EOF

w crates/client/src/webrtc_player.rs <<'EOF'
use anyhow::{Context, Result};
use bytes::BytesMut;
use couchlink_proto::{PadFrame, SignalMessage, PAD_CHANNEL};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

pub struct WebRtcPlayer {
    pub pc: Arc<RTCPeerConnection>,
    pub pad_dc: Arc<tokio::sync::Mutex<Option<Arc<RTCDataChannel>>>>,
}

impl WebRtcPlayer {
    pub async fn new(signal_out: mpsc::UnboundedSender<SignalMessage>) -> Result<Self> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();
        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec![],
                ..Default::default()
            }],
            ..Default::default()
        };
        let pc = Arc::new(api.new_peer_connection(config).await?);
        let pad_dc = Arc::new(tokio::sync::Mutex::new(None));

        let signal_ice = signal_out.clone();
        pc.on_ice_candidate(Box::new(move |c| {
            let signal_ice = signal_ice.clone();
            Box::pin(async move {
                if let Some(c) = c {
                    let _ = signal_ice.send(SignalMessage::IceCandidate {
                        candidate: c.to_json().await.unwrap_or_default().candidate,
                        sdp_mid: c.sdp_mid.clone(),
                        sdp_mline_index: c.sdp_mline_index.map(|v| v as u16),
                    });
                }
            })
        }));

        let pad_slot = Arc::clone(&pad_dc);
        pc.on_data_channel(Box::new(move |dc| {
            let pad_slot = Arc::clone(&pad_slot);
            Box::pin(async move {
                if dc.label() == PAD_CHANNEL {
                    info!("pad channel attached");
                    *pad_slot.lock().await = Some(dc);
                }
            })
        }));

        pc.on_track(Box::new(move |track, _, _| {
            Box::pin(async move {
                info!("video track received: {}", track.codec().capability.mime_type);
                // Decode/display is left to a viewer frontend or SDL sink in a follow-up.
            })
        }));

        Ok(Self { pc, pad_dc })
    }

    pub async fn handle_offer(&self, sdp: String, signal_out: &mpsc::UnboundedSender<SignalMessage>) -> Result<()> {
        let offer = RTCSessionDescription::offer(sdp)?;
        self.pc.set_remote_description(offer).await?;
        let answer = self.pc.create_answer(None).await?;
        self.pc.set_local_description(answer).await?;
        let local = self.pc.local_description().await.context("local desc")?;
        signal_out.send(SignalMessage::Answer { sdp: local.sdp })?;
        Ok(())
    }

    pub async fn add_ice(&self, candidate: String, mid: Option<String>, mline: Option<u16>) -> Result<()> {
        use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
        self.pc
            .add_ice_candidate(RTCIceCandidateInit {
                candidate,
                sdp_mid: mid,
                sdp_mline_index: mline,
                ..Default::default()
            })
            .await?;
        Ok(())
    }

    pub async fn send_pad(&self, frame: &PadFrame) -> Result<()> {
        let guard = self.pad_dc.lock().await;
        let Some(dc) = guard.as_ref() else {
            return Ok(());
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        dc.send(&bytes::Bytes::from(buf.to_vec())).await?;
        Ok(())
    }
}
EOF

w crates/client/src/main.rs <<'EOF'
mod dualsense_reader;
mod signaling_client;
mod webrtc_player;

use anyhow::Result;
use clap::Parser;
use couchlink_proto::SignalMessage;
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "couchlink-client", about = "Join a couchlink co-play session", version)]
struct Args {
    #[arg(long, env = "COUCHLINK_SIGNALING", default_value = "ws://127.0.0.1:8443/ws")]
    signaling: String,
    #[arg(long, env = "COUCHLINK_SESSION_ID")]
    session_id: String,
    #[arg(long, env = "COUCHLINK_PIN")]
    pin: String,
    /// Poll DualSense and send pad frames even without video decode UI.
    #[arg(long, default_value_t = true)]
    send_pad: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "couchlink_client=info".into()),
        )
        .init();

    let args = Args::parse();
    let mut signaling = signaling_client::SignalingClient::connect(&args.signaling).await?;
    signaling
        .register_player(args.session_id.clone(), args.pin.clone())
        .await?;

    let signal_out = signaling.outbound.clone();
    let player = webrtc_player::WebRtcPlayer::new(signal_out.clone()).await?;

    let mut reader = if args.send_pad {
        Some(dualsense_reader::DualSenseReader::open_first()?)
    } else {
        None
    };

    let mut pad_interval = tokio::time::interval(std::time::Duration::from_millis(4)); // ~250 Hz

    loop {
        tokio::select! {
            msg = signaling.inbound.recv() => {
                match msg {
                    Some(SignalMessage::Offer { sdp }) => {
                        info!("got offer");
                        player.handle_offer(sdp, &signal_out).await?;
                    }
                    Some(SignalMessage::IceCandidate { candidate, sdp_mid, sdp_mline_index }) => {
                        let _ = player.add_ice(candidate, sdp_mid, sdp_mline_index).await;
                    }
                    Some(SignalMessage::StreamInfo { width, height, fps, codec }) => {
                        info!("stream {width}x{height}@{fps} {codec}");
                    }
                    Some(SignalMessage::PeerLeft) => warn!("host left"),
                    None => break,
                    _ => {}
                }
            }
            _ = pad_interval.tick() => {
                if let Some(r) = reader.as_mut() {
                    if let Some(frame) = r.read_frame()? {
                        if let Err(e) = player.send_pad(&frame).await {
                            warn!("send pad: {e}");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
EOF

commit "feat(client): WebRTC player + 250Hz DualSense pad sender" \
  crates/client/src/signaling_client.rs crates/client/src/webrtc_player.rs crates/client/src/main.rs

echo "part B core crates committed"
