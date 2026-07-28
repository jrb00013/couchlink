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
