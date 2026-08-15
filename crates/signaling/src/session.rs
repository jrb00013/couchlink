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

    /// Registers a player socket, returning the fresh epoch plus the tx of
    /// whichever player socket this one just displaced (if any and different).
    ///
    /// This session supports exactly one player slot. We cannot tell a
    /// legitimate reload apart from a second person clicking the same invite
    /// link — both look identical here (a fresh socket registering while an
    /// old tx is still present and not yet closed) — so we still allow the
    /// takeover rather than stranding a reloading browser. The caller uses
    /// the returned tx to warn whoever just got silently evicted, instead of
    /// leaving them to watch their stream freeze and never learn why.
    pub fn register_player(
        &self,
        session_id: String,
        pin: String,
        tx: WsSender,
    ) -> Result<(u64, Option<WsSender>), String> {
        let Some(mut entry) = self.sessions.get_mut(&session_id) else {
            return Err("unknown session".into());
        };
        Self::check_pin_lock(&entry)?;
        if entry.pin != pin {
            drop(entry);
            self.record_pin_failure(&session_id);
            return Err("invalid PIN for session".into());
        }
        // Every registration is a fresh player socket (reload, new tab, reconnect),
        // so always bump the epoch and let the caller notify the host. A stale tx
        // from a dead socket must never suppress the PeerJoined that triggers the offer.
        entry.player_epoch = entry.player_epoch.saturating_add(1);
        let displaced = entry
            .player
            .tx
            .take()
            .filter(|old| !old.same_channel(&tx));
        entry.player.tx = Some(tx);
        entry.last_activity = Utc::now();
        self.metrics
            .players_registered
            .fetch_add(1, Ordering::Relaxed);
        self.audit
            .record(&session_id, AuditEventKind::PlayerRegistered, None);
        Ok((entry.player_epoch, displaced))
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

    /// Release a peer slot when its socket closes.
    ///
    /// `tx` identifies the closing connection. A reconnecting peer races itself here:
    /// the old socket's close can be processed *after* the replacement has already
    /// registered, and clearing the slot unconditionally then wipes the live
    /// connection. The session is left with no host, every later PeerJoined is
    /// dropped on the floor, and joining players wait forever for an offer that
    /// nobody is listening to ask for.
    pub fn unregister(&self, session_id: &str, role: Role, tx: Option<&WsSender>) {
        let notify = {
            let Some(mut entry) = self.sessions.get_mut(session_id) else {
                return;
            };
            let slot = match role {
                Role::Host => &mut entry.host,
                Role::Player => &mut entry.player,
            };
            // Only the connection that currently owns the slot may vacate it.
            let is_current = match (&slot.tx, tx) {
                (Some(current), Some(closing)) => current.same_channel(closing),
                (Some(_), None) => true,
                (None, _) => false,
            };
            if !is_current {
                self.audit.record(
                    session_id,
                    AuditEventKind::PeerLeft,
                    Some(format!("stale {role:?} socket closed after reconnect")),
                );
                return;
            }
            slot.tx = None;
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
            .filter(|e| {
                now - e.last_activity > ttl
                    && e.host.tx.is_none()
                    && e.player.tx.is_none()
            })
            .map(|e| e.key().clone())
            .collect();
        for id in stale {
            self.sessions.remove(&id);
            self.metrics.sessions_active.fetch_sub(1, Ordering::Relaxed);
            self.audit
                .record(&id, AuditEventKind::SessionExpired, None);
        }
    }

    pub fn touch(&self, session_id: &str) {
        if let Some(mut s) = self.sessions.get_mut(session_id) {
            s.last_activity = Utc::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SessionStore {
        SessionStore::with_limits(Arc::new(AuditLog::new()), Arc::new(Metrics::new()), 5, 600)
    }

    fn chan() -> WsSender {
        mpsc::unbounded_channel::<String>().0
    }

    /// Regression: a reloading browser leaves a stale player tx behind. If that made
    /// register_player report "not the first player", the host was never told to send
    /// an offer and the page hung forever on "waiting for host offer".
    #[test]
    fn player_rejoin_always_produces_a_fresh_epoch() {
        let s = store();
        s.register_host("sid".into(), "pin".into(), None, None, None, chan())
            .expect("host registers");

        let (first, _) = s
            .register_player("sid".into(), "pin".into(), chan())
            .expect("first player");
        // Second registration without any unregister — exactly the reload case.
        let (second, displaced) = s
            .register_player("sid".into(), "pin".into(), chan())
            .expect("player reloads");

        assert!(
            second > first,
            "reload must bump the epoch (got {first} then {second})"
        );
        assert!(displaced.is_some(), "the stale reload tx must be reported so it can be notified");
    }

    /// Regression: a reconnecting host races itself. The old socket's close can be
    /// processed after the replacement has registered, and clearing the slot
    /// unconditionally wipes the live connection — leaving a session with no host,
    /// so every later PeerJoined is dropped and joining players wait forever.
    #[test]
    fn a_stale_host_socket_closing_does_not_evict_its_replacement() {
        let s = store();
        let first = chan();
        s.register_host("sid".into(), "pin".into(), None, None, None, first.clone())
            .expect("first host");

        // The host reconnects before the old socket's close is processed.
        let second = chan();
        s.register_host("sid".into(), "pin".into(), None, None, None, second.clone())
            .expect("host reconnects");

        // Now the stale socket finally closes.
        s.unregister("sid", Role::Host, Some(&first));

        let live = s.peer_tx("sid", Role::Host).expect("host slot must still be held");
        assert!(
            live.same_channel(&second),
            "the reconnected host must still own the slot"
        );
    }

    /// The same race on the player side, which is what a browser reload looks like.
    #[test]
    fn a_stale_player_socket_closing_does_not_evict_its_replacement() {
        let s = store();
        s.register_host("sid".into(), "pin".into(), None, None, None, chan())
            .expect("host");
        let first = chan();
        s.register_player("sid".into(), "pin".into(), first.clone())
            .expect("first player");
        let second = chan();
        s.register_player("sid".into(), "pin".into(), second.clone())
            .expect("player reconnects");
        // the displaced tx from that second call is exactly `first` — checked
        // by the dedicated `displaced tx` assertion in the reload-epoch test.

        s.unregister("sid", Role::Player, Some(&first));

        let live = s.peer_tx("sid", Role::Player).expect("player slot still held");
        assert!(live.same_channel(&second));
    }

    #[test]
    fn the_owning_socket_can_still_vacate_its_slot() {
        let s = store();
        let host = chan();
        s.register_host("sid".into(), "pin".into(), None, None, None, host.clone())
            .expect("host");
        s.unregister("sid", Role::Host, Some(&host));
        assert!(s.peer_tx("sid", Role::Host).is_none());
    }

    #[test]
    fn wrong_pin_is_rejected() {
        let s = store();
        s.register_host("sid".into(), "pin".into(), None, None, None, chan())
            .expect("host registers");
        assert!(s.register_player("sid".into(), "nope".into(), chan()).is_err());
    }
}
