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
    pub player_epoch: u64,
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
                player_epoch: 0,
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
    ) -> Result<(bool, u64), String> {
        let Some(mut entry) = self.sessions.get_mut(&session_id) else {
            return Err("unknown session".into());
        };
        Self::check_pin_lock(&entry)?;
        if entry.pin != pin {
            drop(entry);
            self.record_pin_failure(&session_id);
            return Err("invalid PIN for session".into());
        }
        let first_player = entry.player.tx.is_none();
        if first_player {
            entry.player_epoch = entry.player_epoch.saturating_add(1);
        }
        entry.player.tx = Some(tx);
        entry.last_activity = Utc::now();
        self.metrics
            .players_registered
            .fetch_add(1, Ordering::Relaxed);
        self.audit
            .record(&session_id, AuditEventKind::PlayerRegistered, None);
        Ok((first_player, entry.player_epoch))
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
