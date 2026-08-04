# Multiplayer (2+ Remote Pads) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let two or more friends connect to one couchlink host at the same time, each driving their own virtual controller in RPCS3/PCSX2.

**Architecture:** Everything that is currently a single value becomes slot-addressed. The signaling session grows from one player slot to `MAX_PLAYERS`; every player-directed message carries a `slot`; the host keeps a map of slot → `WebRtcHost` and fans one encoded frame out to all of them; the Windows companion plugs one virtual pad per slot and routes each TCP connection to its own. Capture and encode stay single-instance — the expensive work happens once and only the push is per-peer.

**Tech Stack:** Rust (tokio, webrtc-rs, dashmap), TypeScript/React web client, PowerShell + bash helper scripts, ViGEmBus on Windows.

## Global Constraints

- `MAX_PLAYERS = 4`. RPCS3 and PCSX2 both expose 4 controller ports; ViGEmBus supports 4 X360 pads without extra config.
- Slots are `1..=MAX_PLAYERS` and map to **emulator player `slot + 1`** — the host's own physical pad is always emulator P1. Slot 1 → P2, slot 2 → P3.
- Wire compatibility: `slot` is `#[serde(default)]` on every message. An old client that omits it must still work as slot 1.
- Capture and video encode run **once** per host, never per player. Any design that encodes per-peer is wrong.
- No blocking or `.await` on a per-peer send inside the loop that drains capture — see `webrtc_peer.rs` `video_dc_congested()` and commit `e44f1e7` for why.
- Companion TCP port stays `39251`; slot is negotiated in-band, not by port-per-player.
- Every failure path degrades to "that player gets video only" — never take down the session or the other players.

---

## Current single-player assumptions (verified 2026-08-04)

These are the exact things that must change. Each was read in the code, not assumed:

| Location | Assumption |
|---|---|
| `crates/signaling/src/session.rs:25` | `Session { player: PeerSlot, player_epoch: u64 }` — exactly one player |
| `crates/signaling/src/session.rs:165` | `register_player` overwrites `entry.player.tx` — a 2nd player evicts the 1st |
| `crates/signaling/src/session.rs:183` | `relay()` flips Host↔Player; no way to address one of several players |
| `crates/host/src/main.rs:147,352` | one `host: WebRtcHost`, replaced wholesale on rejoin |
| `crates/host/src/main.rs:127` | one `Arc<Mutex<VirtualPad>>` shared by the single peer |
| `crates/ds-vhid/src/backend.rs:16` | `create()` returns one backend; every TCP session drives the same pad |
| `crates/ds-vhid/src/main.rs` | one `serve_tcp` handler per connection, all sharing that backend |

## File Structure

**Created**
- `crates/signaling/src/players.rs` — the slot table: assign, vacate, lookup, count. Pure data + tests, no async.
- `crates/host/src/peers.rs` — slot → `WebRtcHost` registry, plus fan-out helpers.
- `crates/ds-vhid/src/pads.rs` — slot → backend map, one virtual pad per slot.

**Modified**
- `crates/proto/src/signal.rs` — add `slot` to player-directed variants.
- `crates/signaling/src/session.rs` — `player: PeerSlot` → `players: PlayerTable`.
- `crates/signaling/src/ws.rs` — relay by slot.
- `crates/host/src/main.rs` — drive the registry instead of one peer.
- `crates/host/src/webrtc_peer.rs` — `WebRtcHost::new` takes a slot + its own pad handle.
- `crates/host/src/emulator_pad.rs` — bind per slot (already reads `COUCHLINK_EMU_PLAYER`).
- `crates/ds-vhid/src/main.rs`, `session.rs` — slot handshake, per-slot routing.
- `web/src/player.ts`, `web/src/App.tsx` — carry and display the assigned slot.
- `scripts/link-emulator-pad.sh` — already slot-capable via `COUCHLINK_EMU_PLAYER`; no change expected, verify only.

---

### Task 1: Player slot table in signaling

The smallest independent piece: a pure data structure with no async, so it can be fully tested before anything else moves.

**Files:**
- Create: `crates/signaling/src/players.rs`
- Modify: `crates/signaling/src/lib.rs` (or `main.rs`) to add `mod players;`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub const MAX_PLAYERS: u8 = 4;`
  - `pub struct PlayerTable` with `pub fn assign(&mut self, tx: WsSender) -> Option<(u8, u64)>`, `pub fn vacate(&mut self, slot: u8, tx: &WsSender) -> bool`, `pub fn get(&self, slot: u8) -> Option<WsSender>`, `pub fn occupied(&self) -> u8`, `pub fn slots(&self) -> impl Iterator<Item = (u8, WsSender)>`.

- [ ] **Step 1: Write the failing tests**

```rust
// crates/signaling/src/players.rs
#[cfg(test)]
mod tests {
    use super::*;

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
        let (s1, _) = t.assign(tx()).unwrap();
        let first = t.get(s1).unwrap();
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
        // Regression: today register_player silently evicts the sitting player.
        assert!(t.assign(tx()).is_none());
        assert_eq!(t.occupied(), MAX_PLAYERS);
    }

    #[test]
    fn a_stale_socket_cannot_vacate_the_live_one() {
        let mut t = PlayerTable::default();
        let stale = tx();
        let (slot, _) = t.assign(stale.clone()).unwrap();
        t.vacate(slot, &stale);
        let live = tx();
        t.assign(live.clone()).unwrap();
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
}
```

Provide a `fn tx() -> WsSender` test helper that returns the sender half of an
`mpsc::unbounded_channel::<String>()`, matching the existing `WsSender` alias in
`session.rs`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p couchlink-signaling players`
Expected: FAIL — `PlayerTable` not found.

- [ ] **Step 3: Implement `PlayerTable`**

```rust
//! Slot table for connected players.
//!
//! Split out from Session because the eviction and stale-socket rules are the
//! subtle part of multiplayer and deserve tests that do not need a websocket.

use crate::session::WsSender;

pub const MAX_PLAYERS: u8 = 4;

#[derive(Default)]
pub struct PlayerTable {
    slots: [Option<WsSender>; MAX_PLAYERS as usize],
    epoch: u64,
}

impl PlayerTable {
    /// Lowest free slot, or None when full. Never evicts a sitting player.
    pub fn assign(&mut self, tx: WsSender) -> Option<(u8, u64)> {
        let idx = self.slots.iter().position(|s| s.is_none())?;
        self.slots[idx] = Some(tx);
        self.epoch = self.epoch.saturating_add(1);
        Some((idx as u8 + 1, self.epoch))
    }

    /// Only the socket that currently owns the slot may release it — a reloading
    /// browser's dead socket must not wipe the new one it just opened.
    pub fn vacate(&mut self, slot: u8, tx: &WsSender) -> bool {
        let Some(cur) = self.slot_mut(slot) else { return false };
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

    pub fn occupied(&self) -> u8 {
        self.slots.iter().filter(|s| s.is_some()).count() as u8
    }

    pub fn slots(&self) -> impl Iterator<Item = (u8, WsSender)> + '_ {
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
```

`same_channel` is `tokio::sync::mpsc::UnboundedSender::same_channel`. If the
existing `WsSender` is a different type, compare with `Arc::ptr_eq` or add an id
field — do **not** compare by value, the stale-socket test exists to catch that.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p couchlink-signaling players`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/signaling/src/players.rs crates/signaling/src/lib.rs
git commit -m "feat(signaling): slot table for multiple players"
```

---

### Task 2: Slot-addressed protocol

**Files:**
- Modify: `crates/proto/src/signal.rs`
- Modify: `web/src/proto.ts`
- Test: `crates/proto/src/signal.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `MAX_PLAYERS` from Task 1 (for validation only).
- Produces: `slot: u8` field on `Registered`, `PeerJoined`, `PeerLeft`, `Offer`, `Answer`, `IceCandidate`, `PadInfo`. All `#[serde(default)]`, absent meaning slot 1.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn slot_defaults_to_one_for_old_clients() {
    // Wire compatibility: a player build from before multiplayer omits `slot`
    // entirely. Defaulting to 0 would address a slot that cannot exist.
    let m: SignalMessage = serde_json::from_str(
        r#"{"type":"answer","sdp":"x","epoch":3}"#,
    ).unwrap();
    match m {
        SignalMessage::Answer { slot, .. } => assert_eq!(slot, 1),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn slot_round_trips() {
    let m = SignalMessage::PeerJoined { role: Role::Player, epoch: 1, slot: 3 };
    let s = serde_json::to_string(&m).unwrap();
    assert!(s.contains("\"slot\":3"));
    assert_eq!(serde_json::from_str::<SignalMessage>(&s).unwrap(), m);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p couchlink-proto slot`
Expected: FAIL — no `slot` field.

- [ ] **Step 3: Add the field**

```rust
/// Player slot, 1..=MAX_PLAYERS. Emulator player number is `slot + 1` —
/// the host's own physical pad owns P1.
#[serde(default = "default_slot")]
pub slot: u8,

fn default_slot() -> u8 { 1 }
```

Add to `Registered`, `PeerJoined`, `PeerLeft`, `Offer`, `Answer`,
`IceCandidate`, `PadInfo`. Mirror in `web/src/proto.ts` as `slot?: number`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p couchlink-proto && cd web && npx tsc --noEmit`
Expected: PASS, typecheck clean.

- [ ] **Step 5: Commit**

```bash
git add crates/proto/src/signal.rs web/src/proto.ts
git commit -m "feat(proto): address signaling messages by player slot"
```

---

### Task 3: Signaling relays by slot

**Files:**
- Modify: `crates/signaling/src/session.rs:25,146,183`
- Modify: `crates/signaling/src/ws.rs:91,155-167`

**Interfaces:**
- Consumes: `PlayerTable` (Task 1), `slot` field (Task 2).
- Produces: `SessionStore::register_player(...) -> Result<(u8, u64), String>` (now returns the slot), `SessionStore::relay_to_player(&self, sid: &str, slot: u8, msg: &str)`, `SessionStore::relay_to_host(&self, sid: &str, msg: &str)`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn second_player_gets_its_own_slot_and_the_first_survives() {
    let store = SessionStore::new(/* existing args */);
    store.register_host("s".into(), "1234".into(), None, None, None).unwrap();
    let (s1, _) = store.register_player("s".into(), "1234".into(), tx_a()).unwrap();
    let (s2, _) = store.register_player("s".into(), "1234".into(), tx_b()).unwrap();
    assert_eq!((s1, s2), (1, 2));
    // Regression: the old single-slot store evicted player 1 here.
    assert!(store.peer_tx_for_slot("s", 1).is_some());
}

#[test]
fn host_message_reaches_only_the_addressed_player() {
    // ... register host + two players with distinguishable receivers ...
    store.relay_to_player("s", 2, "hello");
    assert!(rx_b.try_recv().is_ok());
    assert!(rx_a.try_recv().is_err());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p couchlink-signaling relay`
Expected: FAIL — `relay_to_player` not found.

- [ ] **Step 3: Implement**

Replace `player: PeerSlot` / `player_epoch` in `Session` with
`players: PlayerTable`. In `register_player`, return
`entry.players.assign(tx).ok_or("session full")?`. Split `relay` into
`relay_to_host` (any player → the host slot) and `relay_to_player` (host →
one slot). In `ws.rs`, keep the sender's slot in a local `player_slot`
variable at registration and stamp it onto every message that variant carries
before relaying, so the host always learns who spoke. When the host sends
`Offer`/`IceCandidate`, route by the message's own `slot`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p couchlink-signaling`
Expected: PASS, including the pre-existing stale-socket regression tests at `session.rs:285`.

- [ ] **Step 5: Commit**

```bash
git add crates/signaling/src
git commit -m "feat(signaling): relay to a specific player slot"
```

---

### Task 4: One virtual pad per slot in the companion

Independent of the host work — can be developed and tested on Windows alone.

**Files:**
- Create: `crates/ds-vhid/src/pads.rs`
- Modify: `crates/ds-vhid/src/backend.rs:16`, `crates/ds-vhid/src/session.rs:39`, `crates/ds-vhid/src/main.rs`

**Interfaces:**
- Consumes: existing `PadBackend` trait, `backend::create`.
- Produces: `pub struct PadRegistry` with `pub fn for_slot(&self, slot: u8) -> Result<Arc<Mutex<dyn PadBackend>>>`, lazily plugging a pad the first time a slot is used.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn each_slot_gets_a_distinct_backend() {
    let reg = PadRegistry::new(BackendKind::Noop, MAX_PADS);
    let a = reg.for_slot(1).unwrap();
    let b = reg.for_slot(2).unwrap();
    // Regression: every TCP session used to share one pad, so two players
    // moved the same stick.
    assert!(!Arc::ptr_eq(&a, &b));
}

#[test]
fn the_same_slot_returns_the_same_backend() {
    let reg = PadRegistry::new(BackendKind::Noop, MAX_PADS);
    assert!(Arc::ptr_eq(&reg.for_slot(1).unwrap(), &reg.for_slot(1).unwrap()));
}

#[test]
fn slots_beyond_capacity_are_refused() {
    let reg = PadRegistry::new(BackendKind::Noop, 2);
    assert!(reg.for_slot(3).is_err());
}
```

Add a `BackendKind::Noop` that plugs nothing, so these run on Linux CI without
ViGEmBus.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p couchlink-ds-vhid pads`
Expected: FAIL — `PadRegistry` not found.

- [ ] **Step 3: Implement the registry and the slot handshake**

`PadRegistry` holds `Mutex<HashMap<u8, Arc<Mutex<dyn PadBackend>>>>` and calls
the existing `backend::create` per slot. Extend the wire protocol with a first
frame from the host: `[0xC1, slot]`. `serve_tcp` reads it, resolves
`registry.for_slot(slot)`, and uses that backend for the rest of the
connection. A connection that sends no handshake defaults to slot 1 so an old
host still works.

Log the plug line with its slot: `ViGEm Xbox 360 plugged (P{})`, `slot + 1`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p couchlink-ds-vhid`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/ds-vhid/src
git commit -m "feat(ds-vhid): one virtual pad per player slot"
```

---

### Task 5: Host peer registry with single-encode fan-out

The task most likely to regress latency — keep the encode path untouched and
only make the push per-peer.

**Files:**
- Create: `crates/host/src/peers.rs`
- Modify: `crates/host/src/main.rs:147,343,352,421-490`
- Modify: `crates/host/src/webrtc_peer.rs:126`

**Interfaces:**
- Consumes: `slot` (Task 2), companion handshake (Task 4).
- Produces: `pub struct PeerRegistry` with `pub fn insert(&mut self, slot: u8, host: WebRtcHost)`, `pub fn remove(&mut self, slot: u8)`, `pub fn get(&self, slot: u8) -> Option<&WebRtcHost>`, `pub fn count(&self) -> usize`, `pub async fn push_h264_all(&self, nal: Vec<u8>, dur: Duration, keyframe: bool)`, `pub fn any_keyframe_request(&self) -> bool`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn count_tracks_joins_and_leaves() {
    let mut reg = PeerRegistry::default();
    assert_eq!(reg.count(), 0);
    reg.insert(1, stub_host());
    reg.insert(2, stub_host());
    assert_eq!(reg.count(), 2);
    reg.remove(1);
    assert_eq!(reg.count(), 1);
    // Slot 2 must survive slot 1 leaving.
    assert!(reg.get(2).is_some());
}

#[test]
fn replacing_a_slot_does_not_change_the_count() {
    let mut reg = PeerRegistry::default();
    reg.insert(1, stub_host());
    reg.insert(1, stub_host()); // reload / rejoin
    assert_eq!(reg.count(), 1);
}
```

`stub_host()` needs `WebRtcHost` construction without a real peer connection —
if that is impractical, define `PeerRegistry` over a `trait PeerSink` that
`WebRtcHost` implements, and test against a stub implementing `PeerSink`.
Prefer the trait: it keeps these tests off the network.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p couchlink-host peers`
Expected: FAIL — `PeerRegistry` not found.

- [ ] **Step 3: Implement, and rewire the frame loop**

In `main.rs`, replace `let mut host = WebRtcHost::new(...)` with a
`PeerRegistry`. In the cadence branch, the H.264 block currently ends in
`host.push_h264(...)`. Change **only** the push:

```rust
// Encode once, push N times. Anything above this line must stay per-host —
// re-encoding per player would multiply the most expensive work in the loop.
reg.push_h264_all(nal, per_frame, keyframe).await;
```

`push_h264_all` must not let one slow peer stall the others or the capture
drain. Send to all peers concurrently and bound the wait:

```rust
pub async fn push_h264_all(&self, nal: Vec<u8>, dur: Duration, keyframe: bool) {
    let sends = self.peers.iter().map(|(slot, h)| {
        let nal = nal.clone();
        async move {
            if let Err(e) = h.push_h264(nal, dur, keyframe).await {
                warn!("push h264 to slot {slot}: {e}");
            }
        }
    });
    futures::future::join_all(sends).await;
}
```

`take_keyframe_request` becomes `any_keyframe_request` — one player asking for
an IDR gets one for everybody, which is correct because the encoder is shared.

Handle `PeerJoined { slot }` by building a `WebRtcHost` for that slot and
inserting it; `PeerLeft { slot }` by removing it. Only call `capturer.resync()`
when the registry goes from 0 to 1 — resyncing on every join would jolt the
players already watching.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p couchlink-host && cargo build --workspace --release`
Expected: PASS, clean build.

- [ ] **Step 5: Commit**

```bash
git add crates/host/src/peers.rs crates/host/src/main.rs crates/host/src/webrtc_peer.rs
git commit -m "feat(host): per-slot peer registry with single-encode fan-out"
```

---

### Task 6: Per-slot pad routing and emulator binding

**Files:**
- Modify: `crates/host/src/webrtc_peer.rs:126,232-264`
- Modify: `crates/host/src/emulator_pad.rs`
- Verify: `scripts/link-emulator-pad.sh` (already reads `COUCHLINK_EMU_PLAYER`)

**Interfaces:**
- Consumes: `PadRegistry` handshake (Task 4), `PeerRegistry` (Task 5).
- Produces: `WebRtcHost::new(..., slot: u8, ...)`; `emulator_pad::apply(kind: &str, id: &str, slot: u8)`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn slot_maps_to_the_next_emulator_player() {
    // The host's own physical pad is always emulator P1.
    assert_eq!(emulator_player_for(1), 2);
    assert_eq!(emulator_player_for(2), 3);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p couchlink-host emulator`
Expected: FAIL — `emulator_player_for` not found.

- [ ] **Step 3: Implement**

```rust
/// Emulator player number for a couchlink slot. The host's physical pad owns
/// P1, so remote slot 1 is the emulator's P2.
pub fn emulator_player_for(slot: u8) -> u8 { slot + 1 }
```

`apply` gains a `slot` parameter and passes
`COUCHLINK_EMU_PLAYER=emulator_player_for(slot)` to
`scripts/link-emulator-pad.sh`, alongside the existing
`COUCHLINK_DS_VHID_BACKEND`. Each `WebRtcHost` opens its own companion
connection with `[0xC1, slot]` so its pad frames land on that slot's pad.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p couchlink-host`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/host/src crates/pad/src
git commit -m "feat(host): route each player's pad to its own slot and emulator port"
```

---

### Task 7: Client shows its slot; host reports the count

**Files:**
- Modify: `web/src/player.ts` (store `slot` from `registered`, stamp it on `answer`/`ice_candidate`/`pad_info`)
- Modify: `web/src/App.tsx` (show "Player 2 of 3")
- Test: `web/src/player.test.ts` (new)

**Interfaces:**
- Consumes: `slot` on `Registered` (Task 2).
- Produces: none downstream.

- [ ] **Step 1: Write the failing test**

```ts
it("stamps its assigned slot on outgoing messages", () => {
  const p = new CouchlinkPlayer(cbs);
  p.handleSignal({ type: "registered", slot: 3 } as SignalMessage);
  expect(p.assignedSlot).toBe(3);
});

it("defaults to slot 1 when the host predates multiplayer", () => {
  const p = new CouchlinkPlayer(cbs);
  p.handleSignal({ type: "registered" } as SignalMessage);
  expect(p.assignedSlot).toBe(1);
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd web && npx vitest run src/player.test.ts`
Expected: FAIL — `assignedSlot` undefined.

- [ ] **Step 3: Implement**

Store `msg.slot ?? 1` on `registered`, include it in every `send()` for
`answer`, `ice_candidate`, and `pad_info`. Surface it in the UI header.

- [ ] **Step 4: Run to verify it passes**

Run: `cd web && npx vitest run && npx tsc --noEmit && npm run build`
Expected: PASS, typecheck clean, bundle built.

- [ ] **Step 5: Commit**

```bash
git add web/src web/dist
git commit -m "feat(web): show the player's assigned slot"
```

---

### Task 8: Two-player end-to-end verification and docs

**Files:**
- Modify: `docs/EMULATORS.md`, `docs/ROADMAP.md`, `README.md`
- Create: `scripts/test-multiplayer.sh`

- [ ] **Step 1: Write the smoke script**

```bash
#!/usr/bin/env bash
# Two headless players against a running host; assert distinct slots and pads.
set -euo pipefail
# 1. start host --local
# 2. connect two clients with the same session+pin
# 3. assert the host log shows "players: 2"
# 4. assert the companion log shows two "plugged (P2)" / "(P3)" lines
```

- [ ] **Step 2: Run it against a live host**

Run: `./scripts/test-multiplayer.sh`
Expected: two slots assigned, two pads plugged, neither client evicted.

- [ ] **Step 3: Manual check in RPCS3**

Open RPCS3 → Pads. Player 2 and Player 3 must both light up, each from the
matching friend's controller.

- [ ] **Step 4: Update docs**

Tick "Multi-player (2+ remote pads)" in `docs/ROADMAP.md`. Document the slot →
emulator-player mapping and `MAX_PLAYERS` in `docs/EMULATORS.md`. Fix
`README.md:101`, which still says to bind P2 to "DualSense Wireless Controller".

- [ ] **Step 5: Commit**

```bash
git add docs README.md scripts/test-multiplayer.sh
git commit -m "docs: multiplayer slots and verification"
```

---

## Risks

**Upstream bandwidth is the real ceiling.** Each extra player is another full
copy of the encoded stream out of a residential uplink. At the 720p60 preset
(10 Mbps) two players need ~20 Mbps up, which is beyond most cable plans —
including the Spectrum line this was developed on. Expect to drop to 720p30 for
2+ players. Consider making the preset adapt to `reg.count()` in a follow-up;
it is deliberately **not** in this plan because it needs its own measurement.

**Fan-out amplifies the stall bug fixed in `e44f1e7`.** One congested player
must never block the others. `push_h264_all` uses `join_all`, and the
`video_dc_congested()` guard already sheds per-peer — verify under a
deliberately throttled second client before calling this done.

**ViGEmBus pad ordering is not guaranteed.** XInput slot numbering is assigned
by Windows, so pad 1 and pad 2 may not land in the order they were plugged.
Task 4 must log the actual index and Task 6 must bind from that, not from an
assumption.

**Test coverage is thinner than it looks.** Tasks 4-6 touch Windows-only code
paths that CI cannot exercise. The `Noop` backend keeps the logic testable, but
the ViGEm plug path only gets covered by Task 8's manual check.
