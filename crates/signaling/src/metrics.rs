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
