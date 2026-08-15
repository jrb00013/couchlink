//! Slot table for connected players.
//!
//! Split out of `Session` because the eviction and stale-socket rules are the
//! subtle part of multiplayer and deserve tests that do not need a websocket.
//!
//! Slots are 1-based and map to **emulator player `slot + 1`** — the host's own
//! physical pad always owns P1, so remote slot 1 is the emulator's P2.

use crate::session::WsSender;

/// Three remote slots + the host's own pad = 4 controllers total, matching the
/// four controller ports RPCS3 and PCSX2 expose. Remote slot `s` is emulator
/// port `s + 1`; the host owns P1.
pub const MAX_PLAYERS: u8 = 3;

/// Emulator player number for a couchlink slot.
pub fn emulator_player_for(slot: u8) -> u8 {
    slot + 1
}

#[derive(Default)]
pub struct PlayerTable {
    slots: [Option<WsSender>; MAX_PLAYERS as usize],
    epoch: u64,
}

impl PlayerTable {
    /// Lowest free slot, or `None` when full.
    ///
    /// Never evicts a sitting player: the single-slot store this replaces
    /// overwrote whoever was already connected, so a second friend joining
    /// silently kicked the first.
    pub fn assign(&mut self, tx: WsSender) -> Option<(u8, u64)> {
        let idx = self.slots.iter().position(|s| s.is_none())?;
        self.slots[idx] = Some(tx);
        self.epoch = self.epoch.saturating_add(1);
        Some((idx as u8 + 1, self.epoch))
    }

    /// Re-register a socket that already holds a slot (a duplicate `RegisterPlayer`
    /// on the same connection). Keeps the slot instead of consuming a second one —
    /// otherwise the old slot would never be vacated (the socket's close only
    /// releases its *current* slot) and would leak.
    pub fn reassert(&mut self, tx: &WsSender) -> Option<(u8, u64)> {
        let slot = self.slot_of(tx)?;
        self.epoch = self.epoch.saturating_add(1);
        Some((slot, self.epoch))
    }

    /// Release a slot, but only for the socket that currently owns it.
    ///
    /// A reloading browser leaves a dead socket behind that closes *after* the
    /// new one has registered. Clearing unconditionally would wipe the live
    /// connection and strand the player waiting for an offer.
    pub fn vacate(&mut self, slot: u8, tx: &WsSender) -> bool {
        let Some(cur) = self.slot_mut(slot) else {
            return false;
        };
        match cur {
            Some(existing) if existing.same_channel(tx) => {
                *cur = None;
                true
            }
            _ => false,
        }
    }

    pub fn get(&self, slot: u8) -> Option<WsSender> {
        self.slot(slot).and_then(|s| s.clone())
    }

    /// Slot currently held by this socket, if any.
    pub fn slot_of(&self, tx: &WsSender) -> Option<u8> {
        self.slots
            .iter()
            .position(|s| matches!(s, Some(e) if e.same_channel(tx)))
            .map(|i| i as u8 + 1)
    }

    pub fn occupied(&self) -> u8 {
        self.slots.iter().filter(|s| s.is_some()).count() as u8
    }

    pub fn is_empty(&self) -> bool {
        self.occupied() == 0
    }

    /// Every connected player, as `(slot, tx)`.
    pub fn iter(&self) -> impl Iterator<Item = (u8, WsSender)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.clone().map(|tx| (i as u8 + 1, tx)))
    }

    fn slot(&self, slot: u8) -> Option<&Option<WsSender>> {
        self.slots.get(slot.checked_sub(1)? as usize)
    }

    fn slot_mut(&mut self, slot: u8) -> Option<&mut Option<WsSender>> {
        self.slots.get_mut(slot.checked_sub(1)? as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn tx() -> WsSender {
        mpsc::unbounded_channel::<String>().0
    }

    #[test]
    fn assigns_lowest_free_slot() {
        let mut t = PlayerTable::default();
        let (a, _) = t.assign(tx()).unwrap();
        let (b, _) = t.assign(tx()).unwrap();
        assert_eq!((a, b), (1, 2));
    }

    #[test]
    fn reuses_a_vacated_slot_rather_than_growing() {
        let mut t = PlayerTable::default();
        let first = tx();
        let (s1, _) = t.assign(first.clone()).unwrap();
        t.assign(tx()).unwrap();
        assert!(t.vacate(s1, &first));
        // Slot 1 is free again and must be handed out before slot 3.
        assert_eq!(t.assign(tx()).unwrap().0, 1);
    }

    #[test]
    fn refuses_to_overbook() {
        let mut t = PlayerTable::default();
        for _ in 0..MAX_PLAYERS {
            assert!(t.assign(tx()).is_some());
        }
        // Regression: the single-slot store evicted the sitting player here.
        assert!(t.assign(tx()).is_none());
        assert_eq!(t.occupied(), MAX_PLAYERS);
    }

    #[test]
    fn a_stale_socket_cannot_vacate_the_live_one() {
        let mut t = PlayerTable::default();
        let stale = tx();
        let (slot, _) = t.assign(stale.clone()).unwrap();
        assert!(t.vacate(slot, &stale));
        let live = tx();
        assert_eq!(t.assign(live.clone()).unwrap().0, slot);
        // The reloaded browser's old socket closing must not wipe its new one.
        assert!(!t.vacate(slot, &stale));
        assert!(t.get(slot).is_some());
    }

    #[test]
    fn epoch_increases_every_assignment() {
        let mut t = PlayerTable::default();
        let (_, e1) = t.assign(tx()).unwrap();
        let (_, e2) = t.assign(tx()).unwrap();
        assert!(e2 > e1);
    }

    #[test]
    fn slot_of_finds_the_owner_and_ignores_strangers() {
        let mut t = PlayerTable::default();
        t.assign(tx()).unwrap();
        let mine = tx();
        let (slot, _) = t.assign(mine.clone()).unwrap();
        assert_eq!(t.slot_of(&mine), Some(slot));
        assert_eq!(t.slot_of(&tx()), None);
    }

    #[test]
    fn iter_yields_only_occupied_slots() {
        let mut t = PlayerTable::default();
        let a = tx();
        t.assign(a.clone()).unwrap();
        t.assign(tx()).unwrap();
        t.vacate(1, &a);
        let slots: Vec<u8> = t.iter().map(|(s, _)| s).collect();
        assert_eq!(slots, vec![2]);
    }

    #[test]
    fn slots_map_onto_emulator_ports_after_the_host_pad() {
        // The host's own controller is always emulator P1.
        assert_eq!(emulator_player_for(1), 2);
        assert_eq!(emulator_player_for(2), 3);
        assert_eq!(emulator_player_for(3), 4);
        // The host's pad + MAX_PLAYERS remote pads exactly fills 4 emulator ports.
        assert_eq!(emulator_player_for(MAX_PLAYERS), 4);
    }
}
