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
    /// Up to 3 concurrent player slots; never evicts a sitting player.
    pub players: crate::players::PlayerTable,
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
                players: crate::players::PlayerTable::default(),
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

    /// Registers a player socket into the first free slot, returning `(slot, epoch)`.
    ///
    /// Never evicts a sitting player: once all 3 slots are full, further joins are
    /// rejected with a "session full" error rather than kicking someone off (the
    /// single-slot store this replaces overwrote whoever was connected, so a second
    /// friend joining silently kicked the first).
    ///
    /// A reloading browser's *new* socket registers at a fresh slot while the stale
    /// one still holds the old slot; the stale socket's close then vacates it via
    /// `unregister`. The host is told every re-registration so it always rebuilds
    /// for the newest slot/epoch.
    pub fn register_player(
        &self,
        session_id: String,
        pin: String,
        tx: WsSender,
    ) -> Result<(u8, u64), String> {
        let Some(mut entry) = self.sessions.get_mut(&session_id) else {
            return Err("unknown session".into());
        };
        Self::check_pin_lock(&entry)?;
        if entry.pin != pin {
            drop(entry);
            self.record_pin_failure(&session_id);
            return Err("invalid PIN for session".into());
        }
        let max = crate::players::MAX_PLAYERS;
        let (slot, epoch) = match entry.players.slot_of(&tx) {
            // Duplicate RegisterPlayer on the same connection: keep its slot.
            Some(_) => entry.players.reassert(&tx),
            None => entry.players.assign(tx),
        }
        .ok_or_else(|| format!("session full ({max}/{max})"))?;
        entry.last_activity = Utc::now();
        self.metrics
            .players_registered
            .fetch_add(1, Ordering::Relaxed);
        self.audit
            .record(&session_id, AuditEventKind::PlayerRegistered, None);
        Ok((slot, epoch))
    }

    pub fn peer_tx(&self, session_id: &str, role: Role) -> Option<WsSender> {
        let entry = self.sessions.get(session_id)?;
        match role {
            Role::Host => entry.host.tx.clone(),
            // Legacy single-player lookup — prefer `player_tx(slot)` when the
            // slot is known (with multiple players this is ambiguous).
            Role::Player => entry.players.iter().next().map(|(_, tx)| tx),
        }
    }

    /// The player socket holding `slot`, or the sole player when `slot == 0`
    /// (a pre-slot host that never stamped one). `None` when the slot is free.
    pub fn player_tx(&self, session_id: &str, slot: u8) -> Option<WsSender> {
        let entry = self.sessions.get(session_id)?;
        if slot == 0 {
            let mut it = entry.players.iter();
            let first = it.next().map(|(_, tx)| tx);
            if it.next().is_some() {
                None // ambiguous with 2+ players; never guess
            } else {
                first
            }
        } else {
            entry.players.get(slot)
        }
    }

    /// Send `msg` to every connected player.
    pub fn broadcast_to_players(&self, session_id: &str, msg: &str) {
        let Some(entry) = self.sessions.get(session_id) else {
            return;
        };
        for (_, tx) in entry.players.iter() {
            let _ = tx.send(msg.to_string());
        }
    }

    /// Send `msg` to the host and every connected player.
    pub fn broadcast(&self, session_id: &str, msg: &str) {
        if let Some(tx) = self.peer_tx(session_id, Role::Host) {
            let _ = tx.send(msg.to_string());
        }
        self.broadcast_to_players(session_id, msg);
    }

    /// Current occupancy `(occupied, max)` for the `PlayersStatus` broadcast.
    pub fn players_status(&self, session_id: &str) -> Option<(u8, u8)> {
        let entry = self.sessions.get(session_id)?;
        Some((entry.players.occupied(), crate::players::MAX_PLAYERS))
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
        enum WhoLeft {
            Host,
            Player(u8),
        }
        let who = {
            let Some(mut entry) = self.sessions.get_mut(session_id) else {
                return;
            };
            match role {
                Role::Host => {
                    // Only the connection that currently owns the host slot may vacate it.
                    let is_current = match (&entry.host.tx, tx) {
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
                    entry.host.tx = None;
                    entry.last_activity = Utc::now();
                    WhoLeft::Host
                }
                Role::Player => {
                    // Find the slot this socket owns; a socket that owns no slot
                    // (a reload's stale socket, already superseded) is a no-op.
                    let Some(closing) = tx else {
                        return;
                    };
                    let Some(slot) = entry.players.slot_of(closing) else {
                        self.audit.record(
                            session_id,
                            AuditEventKind::PeerLeft,
                            Some(format!("stale {role:?} socket closed after reconnect")),
                        );
                        return;
                    };
                    if !entry.players.vacate(slot, closing) {
                        return;
                    }
                    entry.last_activity = Utc::now();
                    WhoLeft::Player(slot)
                }
            }
        };
        match who {
            WhoLeft::Host => {
                let peer_left = couchlink_proto::SignalMessage::PeerLeft { slot: 0 }
                    .to_json()
                    .unwrap_or_default();
                self.broadcast_to_players(session_id, &peer_left);
            }
            WhoLeft::Player(slot) => {
                // Name the slot so the host can tear down exactly that peer
                // connection instead of guessing when more than one is up.
                // Also reaches every other player (not just the host) so a
                // controller debug view can drop that slot's pad info instead
                // of showing a stale "connected" for someone who just left.
                let peer_left = couchlink_proto::SignalMessage::PeerLeft { slot }
                    .to_json()
                    .unwrap_or_default();
                if let Some(tx) = self.peer_tx(session_id, Role::Host) {
                    let _ = tx.send(peer_left.clone());
                }
                self.broadcast_to_players(session_id, &peer_left);
            }
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
                    && e.players.is_empty()
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
    ///
    /// Under the slot table a reload's *new* socket registers at a fresh slot while
    /// the stale one still holds the old slot, so the host is still told (new epoch,
    /// new slot) and the stale socket's later close vacates the old slot.
    #[test]
    fn player_reload_always_produces_a_fresh_epoch() {
        let s = store();
        s.register_host("sid".into(), "pin".into(), None, None, None, chan())
            .expect("host registers");

        let (slot1, first) = s
            .register_player("sid".into(), "pin".into(), chan())
            .expect("first player");
        // Second registration without any unregister — exactly the reload case.
        let (slot2, second) = s
            .register_player("sid".into(), "pin".into(), chan())
            .expect("player reloads");

        assert!(
            second > first,
            "reload must bump the epoch (got {first} then {second})"
        );
        assert_ne!(
            slot2, slot1,
            "the reloaded socket must take a fresh slot while the stale one is registered"
        );
        assert_eq!(s.players_status("sid"), Some((2, crate::players::MAX_PLAYERS)));
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

    /// The same race on the player side, which is what a browser reload looks like:
    /// the stale socket's close must not wipe the reloaded socket's slot.
    #[test]
    fn a_stale_player_socket_closing_does_not_evict_its_replacement() {
        let s = store();
        s.register_host("sid".into(), "pin".into(), None, None, None, chan())
            .expect("host");
        let first = chan();
        let (slot1, _) = s
            .register_player("sid".into(), "pin".into(), first.clone())
            .expect("first player");
        let second = chan();
        let (slot2, _) = s
            .register_player("sid".into(), "pin".into(), second.clone())
            .expect("player reconnects");
        assert_ne!(slot1, slot2);

        // The stale socket finally closes.
        s.unregister("sid", Role::Player, Some(&first));

        let live = s.player_tx("sid", slot2).expect("reload slot still held");
        assert!(live.same_channel(&second));
        assert!(
            s.player_tx("sid", slot1).is_none(),
            "the stale socket's old slot must be vacated by its close"
        );
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

    #[test]
    fn two_concurrent_players_both_keep_their_slots() {
        let s = store();
        s.register_host("sid".into(), "pin".into(), None, None, None, chan())
            .expect("host");
        let a = chan();
        let (slot_a, _) = s
            .register_player("sid".into(), "pin".into(), a.clone())
            .expect("player A");
        let b = chan();
        let (slot_b, _) = s
            .register_player("sid".into(), "pin".into(), b.clone())
            .expect("player B");

        assert_eq!((slot_a, slot_b), (1, 2));
        let held_a = s.player_tx("sid", slot_a).expect("A's slot held");
        let held_b = s.player_tx("sid", slot_b).expect("B's slot held");
        assert!(held_a.same_channel(&a));
        assert!(held_b.same_channel(&b));
        assert_eq!(s.players_status("sid"), Some((2, crate::players::MAX_PLAYERS)));
    }

    #[test]
    fn a_player_beyond_max_is_rejected_when_the_table_is_full() {
        let s = store();
        s.register_host("sid".into(), "pin".into(), None, None, None, chan())
            .expect("host");
        // Fill the table (host pad is P1, so only MAX_PLAYERS remote slots).
        for expected_slot in 1..=crate::players::MAX_PLAYERS {
            let (slot, _) = s
                .register_player("sid".into(), "pin".into(), chan())
                .expect("player registers");
            assert_eq!(slot, expected_slot);
        }
        // The next join must be rejected, never evicting a sitting player.
        let err = s
            .register_player("sid".into(), "pin".into(), chan())
            .expect_err("table is full");
        assert!(
            err.contains("session full"),
            "expected a session-full error, got {err:?}"
        );
        assert_eq!(s.players_status("sid").map(|(o, _)| o), Some(crate::players::MAX_PLAYERS));
    }

    #[test]
    fn host_leaving_notifies_every_player() {
        let s = store();
        s.register_host("sid".into(), "pin".into(), None, None, None, chan())
            .expect("host");
        let (a_tx, mut a_rx) = mpsc::unbounded_channel::<String>();
        let (b_tx, mut b_rx) = mpsc::unbounded_channel::<String>();
        s.register_player("sid".into(), "pin".into(), a_tx)
            .expect("player A");
        s.register_player("sid".into(), "pin".into(), b_tx)
            .expect("player B");

        s.unregister("sid", Role::Host, None);

        let a: couchlink_proto::SignalMessage =
            serde_json::from_str(&a_rx.try_recv().expect("A notified")).unwrap();
        let b: couchlink_proto::SignalMessage =
            serde_json::from_str(&b_rx.try_recv().expect("B notified")).unwrap();
        assert!(matches!(a, couchlink_proto::SignalMessage::PeerLeft { .. }));
        assert!(matches!(b, couchlink_proto::SignalMessage::PeerLeft { .. }));
    }
}
