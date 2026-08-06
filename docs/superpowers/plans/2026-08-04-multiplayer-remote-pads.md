# Multiplayer (2+ Remote Pads) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let two or more friends connect to one couchlink host at the same time, each driving their own virtual controller on its own emulator port.

**Architecture:** Everything that is currently a single value becomes slot-addressed. The signaling session grows from one player slot to `MAX_PLAYERS`; every player-directed message carries a `slot`; the host keeps a map of slot → `WebRtcHost` and fans one encoded frame out to all of them; the Windows companion plugs one virtual pad per slot and routes each TCP connection to its own. Capture and encode stay single-instance — the expensive work happens once and only the push is per-peer.

**Tech Stack:** Rust (tokio, webrtc-rs, dashmap), TypeScript/React web client, PowerShell + bash helper scripts, ViGEmBus on Windows.

## Status

| Task | State |
|------|-------|
| 1. Player slot table | **Done** — `2523e35` |
| 2. Slot-addressed protocol | Not started |
| 3. Signaling relays by slot | Not started |
| 4. One virtual pad per slot | Not started |
| 5. Host peer registry + fan-out | Not started |
| 6. Per-slot emulator binding | Partly done — script already takes `COUCHLINK_EMU_PLAYER` |
| 7. Client carries its slot | Not started |
| 8. Two-player verification | Not started |

## Global Constraints

- `MAX_PLAYERS = 4`, already defined in `crates/signaling/src/players.rs`.
- Slots are `1..=MAX_PLAYERS` and map to **emulator player `slot + 1`** — the host's own physical pad is always emulator P1. Use `players::emulator_player_for()`; do not recompute `slot + 1` by hand.
- Wire compatibility: `slot` is `#[serde(default)]` on every message. An old client that omits it must still work as slot 1.
- Capture and video encode run **once** per host, never per player. Any design that encodes per-peer is wrong.
- No unbounded `.await` on a per-peer send inside the loop that drains capture. See `push_bounded()` in `crates/host/src/main.rs`, and commits `e44f1e7` / `0be52e8` / `82c7fd4` for the three separate freezes this caused.
- Companion TCP port stays `39251`; slot is negotiated in-band, not port-per-player.
- Every failure path degrades to "that player gets video only" — never take down the session or the other players.

## Measured facts (2026-08-04)

Established on the development machine. Do not re-derive.

- **Uplink ≈ 35 Mbps.** One player at the 720p60 preset is ~10 Mbps, and the host currently sends every frame **twice** — CLVD DataChannel *and* RTP sample — because it cannot tell which path the viewer paints. Two players at 720p60 is therefore ~40 Mbps of send work against a 35 Mbps pipe.
- **RPCS3 exposes 7 player slots.** Its `Default.yml` contains `Player 1 Input:` through `Player 7 Input:`.
- **PCSX2 exposes 2 controller ports** without multitap. Host + one remote is its ceiling; slots 3 and 4 are RPCS3-only. (An earlier draft of this plan claimed both emulators had 4 ports. That was wrong.)
- **The companion creates one backend** — `crates/ds-vhid/src/backend.rs:16` — shared by every TCP session, so two players today would drive the same virtual stick.

---

## Single-player assumptions still standing

| Location | Assumption |
|---|---|
| `crates/signaling/src/session.rs:25` | `Session { player: PeerSlot, player_epoch: u64 }` — still one player; `PlayerTable` exists but is not wired in |
| `crates/signaling/src/session.rs:165` | `register_player` overwrites `entry.player.tx` — a 2nd player evicts the 1st |
| `crates/signaling/src/session.rs:183` | `relay()` flips Host↔Player; no way to address one of several players |
| `crates/host/src/main.rs:147,352` | one `host: WebRtcHost`, replaced wholesale on rejoin |
| `crates/host/src/main.rs:127` | one `Arc<Mutex<VirtualPad>>` shared by the single peer |
| `crates/ds-vhid/src/backend.rs:16` | `create()` returns one backend; every TCP session drives the same pad |

## File Structure

**Created**
- `crates/host/src/peers.rs` — slot → `WebRtcHost` registry plus fan-out helpers.
- `crates/ds-vhid/src/pads.rs` — slot → backend map, one virtual pad per slot.

**Modified**
- `crates/proto/src/signal.rs` — add `slot` to player-directed variants.
- `crates/signaling/src/session.rs` — `player: PeerSlot` → `players: PlayerTable`.
- `crates/signaling/src/ws.rs` — relay by slot.
- `crates/host/src/main.rs` — drive the registry instead of one peer.
- `crates/host/src/webrtc_peer.rs` — `WebRtcHost::new` takes a slot and its own pad handle.
- `crates/host/src/emulator_pad.rs` — pass the slot through to the script.
- `crates/ds-vhid/src/main.rs`, `session.rs` — slot handshake, per-slot routing.
- `web/src/player.ts`, `web/src/App.tsx` — carry and display the assigned slot.

**Already in place — do not rewrite**
- `crates/signaling/src/players.rs` — `PlayerTable`, `MAX_PLAYERS`, `emulator_player_for()`.
- `scripts/link-emulator-pad.sh` — binds RPCS3 *and* PCSX2, reads `COUCHLINK_EMU_PLAYER`, idempotent, backs up once.

---

### Task 2: Slot-addressed protocol

**Files:**
- Modify: `crates/proto/src/signal.rs`
- Modify: `web/src/proto.ts`
- Test: `crates/proto/src/signal.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: `MAX_PLAYERS` (Task 1, landed).
- Produces: `slot: u8` on `Registered`, `PeerJoined`, `PeerLeft`, `Offer`, `Answer`, `IceCandidate`, `PadInfo`. All `#[serde(default)]`, absent meaning slot 1.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn slot_defaults_to_one_for_old_clients() {
    // Wire compatibility: a player build from before multiplayer omits `slot`
    // entirely. Defaulting to 0 would address a slot that cannot exist.
    let m: SignalMessage =
        serde_json::from_str(r#"{"type":"answer","sdp":"x","epoch":3}"#).unwrap();
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
    // register host + two players with distinguishable receivers
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
`relay_to_host` and `relay_to_player`. In `ws.rs`, capture the sender's slot in
a local at registration and stamp it onto every message that carries one before
relaying, so the host always learns who spoke. When the host sends
`Offer` / `IceCandidate`, route by the message's own `slot`.

The stale-socket regression tests at `session.rs:285` must stay green — they
encode the reload behaviour `PlayerTable::vacate` was built for.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p couchlink-signaling`
Expected: PASS, including the pre-existing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/signaling/src
git commit -m "feat(signaling): relay to a specific player slot"
```

---

### Task 4: One virtual pad per slot in the companion

Independent of the host work — developable and testable on Windows alone.

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
    // Regression: every TCP session shared one pad, so two players moved the
    // same stick.
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
connection. A connection that sends no handshake defaults to slot 1, so an
older host still works.

Log the plug line with the emulator port rather than the raw slot —
`ViGEm Xbox 360 plugged (P{})` with `slot + 1` — and log the **actual** XInput
index Windows assigned, which Task 6 binds from.

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

The task most likely to regress latency — leave the encode path untouched and
make only the push per-peer.

**Files:**
- Create: `crates/host/src/peers.rs`
- Modify: `crates/host/src/main.rs:147,343,352,421-490`
- Modify: `crates/host/src/webrtc_peer.rs:127`

**Interfaces:**
- Consumes: `slot` (Task 2), companion handshake (Task 4).
- Produces: `pub struct PeerRegistry` with `insert(&mut self, slot: u8, host: WebRtcHost)`, `remove(&mut self, slot: u8)`, `get(&self, slot: u8) -> Option<&WebRtcHost>`, `count(&self) -> usize`, `async fn push_h264_all(&self, nal: Vec<u8>, dur: Duration, keyframe: bool)`, `any_keyframe_request(&self) -> bool`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn count_tracks_joins_and_leaves() {
    let mut reg = PeerRegistry::default();
    assert_eq!(reg.count(), 0);
    reg.insert(1, stub_peer());
    reg.insert(2, stub_peer());
    assert_eq!(reg.count(), 2);
    reg.remove(1);
    assert_eq!(reg.count(), 1);
    // Slot 2 must survive slot 1 leaving.
    assert!(reg.get(2).is_some());
}

#[test]
fn replacing_a_slot_does_not_change_the_count() {
    let mut reg = PeerRegistry::default();
    reg.insert(1, stub_peer());
    reg.insert(1, stub_peer()); // reload / rejoin
    assert_eq!(reg.count(), 1);
}
```

`WebRtcHost` cannot be constructed without a real peer connection, so define
`PeerRegistry` over a `trait PeerSink` that `WebRtcHost` implements and test
against a stub. That keeps these tests off the network.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p couchlink-host peers`
Expected: FAIL — `PeerRegistry` not found.

- [ ] **Step 3: Implement, and rewire the frame loop**

In `main.rs`, replace the single `host` with a `PeerRegistry`. In the cadence
branch, change **only** the push:

```rust
// Encode once, push N times. Everything above this line stays per-host —
// re-encoding per player would multiply the most expensive work in the loop.
reg.push_h264_all(nal, per_frame, keyframe).await;
```

One slow peer must not stall the others or the capture drain. Send
concurrently, and keep every peer inside the existing per-frame budget:

```rust
pub async fn push_h264_all(&self, nal: Vec<u8>, dur: Duration, keyframe: bool) {
    let sends = self.peers.iter().map(|(slot, h)| {
        let nal = nal.clone();
        async move {
            if let Err(e) = push_bounded(h, nal, dur, keyframe).await {
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
inserting it; `PeerLeft { slot }` by removing it. Call `capturer.resync()` only
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
- Modify: `crates/host/src/webrtc_peer.rs:127,232-264`
- Modify: `crates/host/src/emulator_pad.rs`
- Verify only: `scripts/link-emulator-pad.sh` (already slot-capable)

**Interfaces:**
- Consumes: `PadRegistry` handshake (Task 4), `PeerRegistry` (Task 5), `players::emulator_player_for`.
- Produces: `WebRtcHost::new(..., slot: u8, ...)`; `emulator_pad::apply(kind: &str, id: &str, slot: u8)`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn apply_targets_the_slots_emulator_port() {
    // Slot 2 is the emulator's P3. Binding it to P2 would steal the other
    // player's pad, and the failure is silent — the button moves the wrong car.
    assert_eq!(emulator_env_for(2), ("3".to_string(), "xbox360".to_string()));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p couchlink-host emulator`
Expected: FAIL — `emulator_env_for` not found.

- [ ] **Step 3: Implement**

`apply` gains a `slot` parameter and passes
`COUCHLINK_EMU_PLAYER=<emulator_player_for(slot)>` to
`scripts/link-emulator-pad.sh` alongside the existing
`COUCHLINK_DS_VHID_BACKEND`. Each `WebRtcHost` opens its own companion
connection with `[0xC1, slot]` so its pad frames land on that slot's pad.

Reuse `players::emulator_player_for` rather than writing `slot + 1` again.

Guard the PCSX2 ceiling: for `slot >= 2`, PCSX2 has no port to bind without
multitap, so skip its half of the script rather than writing a `[Pad4]` block
PCSX2 will ignore.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p couchlink-host`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/host/src
git commit -m "feat(host): route each player's pad to its own slot and emulator port"
```

---

### Task 7: Client carries its slot

**Files:**
- Modify: `web/src/player.ts` (store `slot` from `registered`, stamp it on `answer` / `ice_candidate` / `pad_info`)
- Modify: `web/src/App.tsx` (show "Player 2 of 3")
- Test: `web/src/player.test.ts` (new)

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

Store `msg.slot ?? 1` on `registered`; include it in every `send()` for
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

### Task 8: Two-player verification and docs

**Files:**
- Create: `scripts/test-multiplayer.sh`
- Modify: `docs/EMULATORS.md`, `docs/ROADMAP.md`, `README.md`

- [ ] **Step 1: Write the smoke script**

```bash
#!/usr/bin/env bash
# Two headless players against a running host; assert distinct slots and pads.
set -euo pipefail
# 1. start host --local
# 2. connect two clients with the same session + pin
# 3. assert the host log reports 2 players
# 4. assert the companion log shows two plug lines, "(P2)" and "(P3)"
# 5. assert neither client was evicted
```

- [ ] **Step 2: Run it against a live host**

Run: `./scripts/test-multiplayer.sh`
Expected: two slots assigned, two pads plugged, neither client evicted.

- [ ] **Step 3: Manual check in RPCS3**

Open RPCS3 → Pads. Player 2 and Player 3 must both light up, each from the
matching friend's controller. This is the only step that exercises the real
ViGEm plug path.

- [ ] **Step 4: Update docs**

Tick "Multi-player (2+ remote pads)" in `docs/ROADMAP.md`. Document the slot →
emulator-port mapping, `MAX_PLAYERS`, and the PCSX2 two-port ceiling in
`docs/EMULATORS.md`. Fix `README.md:101`, which still says to bind P2 to
"DualSense Wireless Controller".

- [ ] **Step 5: Commit**

```bash
git add docs README.md scripts/test-multiplayer.sh
git commit -m "docs: multiplayer slots and verification"
```

---

## Risks

**Upstream bandwidth binds before four players do.** Measured uplink is ~35
Mbps; one player at 720p60 is ~10 Mbps and the host currently sends every frame
twice, so two players already need roughly the whole pipe. Expect 720p30 for
2+. Two follow-ups are worth doing before raising the cap, neither in this plan
because each needs its own measurement: stop sending RTP once a viewer reports
the WebCodecs path, and scale the preset with `reg.count()`.

**PCSX2 caps at two ports.** Host plus one remote, unless multitap support is
added. Slots 3 and 4 are RPCS3-only. Do not report "4 players supported"
without naming the emulator.

**Fan-out amplifies the three freezes already fixed.** One congested player
must never block the others or the capture drain. `push_h264_all` keeps each
peer inside `push_bounded`, and keyframes are still never shed. Verify against
a deliberately throttled second client before calling this done.

**ViGEmBus pad ordering is not guaranteed.** XInput slot numbering is assigned
by Windows, so pads may not land in plug order. Task 4 logs the actual index
and Task 6 binds from that, not from an assumption.

**Windows-only paths are untestable in CI.** Tasks 4-6 touch code CI cannot
exercise. The `Noop` backend keeps the logic covered, but the ViGEm plug path
is only checked by Task 8's manual step.
