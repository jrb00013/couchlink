# 4-player support — implementation plan

Status: **scoped, not built**. This document is the plan; nothing here is implemented yet
except the pre-existing, unused `PlayerTable` in `crates/signaling/src/players.rs`.

## Why this is a real project, not a patch

The current host (`crates/host/src/main.rs`, `crates/host/src/webrtc_peer.rs`) is hard-wired
to exactly one `WebRtcHost`: one `RTCPeerConnection`, one video track, one pad DataChannel,
one virtual controller. That single-peer path is ~900 lines of carefully hardened real-time
media code (see recent commits: cold-SCTP-join keyframe warmup, srflx/LAN RTT budget
separation, link-governor backpressure). Turning it into N concurrent peers touches the
capture → encode → push loop, WebRTC peer lifecycle, pad routing, and the wire protocol.
It needs to be built in reviewable stages and verified live against the real host box,
real controllers, and a real emulator — not shipped in one blind rewrite.

## Current state of the pieces

| Piece | State |
|---|---|
| `crates/signaling/src/players.rs` — `PlayerTable`, 4 slots, tested | Built, **not wired in** |
| `crates/signaling/src/session.rs` — `Session.player: PeerSlot` | Single slot, overwrites on every join |
| `crates/signaling/src/ws.rs` — relay | Blind 1:1 host↔player relay, no slot routing |
| `crates/proto/src/signal.rs` — wire protocol | No `slot` field anywhere |
| `crates/host/src/main.rs` / `webrtc_peer.rs` | One `WebRtcHost`, rebuilt on rejoin, never fans out |
| `crates/host/src/emulator_pad.rs` | Binds pad backend globally, no per-slot port targeting |
| Web / native clients | Assume one connection = the session; no slot awareness |

`players.rs`'s own doc comment already states the intended mapping: the host's own
physical pad is emulator port P1; remote slot 1 → P2, slot 2 → P3, slot 3 → P4.
**Open question**: `MAX_PLAYERS = 4` remote slots + the host's own pad = 5 controllers,
but the code comment says most emulators (RPCS3, PCSX2) expose 4 ports total. This needs
resolving against your actual emulator config before Stage 3 — either cap remote slots at
3, or confirm the host doesn't occupy a port.

## Stage 1 — Signaling layer (safe, no hardware, fully unit-testable)

**Goal:** up to 4 players can hold a slot simultaneously without evicting each other.

1. `crates/proto/src/signal.rs`:
   - Add `slot: u8` (`#[serde(default)]` for backward compat) to `PeerJoined`, `Offer`,
     `Answer`, `IceCandidate`, `PadInfo`, `PresentPath`.
   - Convert `RequestOffer` (unit variant) → `RequestOffer { #[serde(default)] slot: u8 }`.
   - Add `slot: u8` to `Registered` (0 for the host role).
   - Add `PlayersStatus { occupied: u8, max: u8 }` — new message, host session status
     broadcast to everyone in the session so clients can show "N/4 players connected".
   - Round-trip tests for every changed/added variant (pattern already established in
     this file's `#[cfg(test)] mod tests`).

2. `crates/signaling/src/session.rs`:
   - Replace `pub player: PeerSlot` with `pub players: crate::players::PlayerTable`.
   - `register_player` calls `players.assign(tx)`; returns `Err("session full (4/4)")`
     when `PlayerTable::assign` returns `None`.
   - `unregister(session_id, Role::Player, tx)` needs the slot to vacate — change its
     signature (or add a `player_slot_of(tx)` lookup) so only the socket that owns a
     slot can vacate it, mirroring the existing stale-socket-can't-evict tests.
   - Add `player_tx(session_id, slot) -> Option<WsSender>` alongside the existing
     `peer_tx`.
   - Update/extend the existing tests in this file's `mod tests` (`player_rejoin_...`,
     `a_stale_player_socket_closing_does_not_evict_its_replacement`) for the new shape,
     and add: `a_fifth_player_is_rejected_when_the_table_is_full`,
     `two_concurrent_players_both_keep_their_slots`.

3. `crates/signaling/src/ws.rs`:
   - Track `player_slot: Option<u8>` per connection (replacing the current bare
     `role: Option<Role>` player case).
   - On `RegisterPlayer`: call `register_player`, get `(slot, epoch)`; reply
     `Registered { role: Player, session_id, slot }`; notify the host with
     `PeerJoined { role: Player, epoch, slot }`; broadcast `PlayersStatus` to the host
     and all connected players.
   - Player → host messages (`Answer`, `IceCandidate`, `PadInfo`, `PresentPath`,
     `RequestOffer`): **stamp the `slot` field from the connection's own known slot**
     before relaying to the host — never trust a client-supplied slot, so one player
     can't spoof messages for another's slot.
   - Host → player messages (`Offer`, `IceCandidate` sent by host): route by the
     message's `slot` field to `player_tx(session_id, slot)` instead of blind relay.
   - On player disconnect: `players.vacate(slot, tx)`; broadcast updated
     `PlayersStatus`.

**Stage 1 exit criteria:** `cargo test -p couchlink-signaling` green, including new
multi-slot tests. Not user-visible yet — the host still only answers slot 1.

## Stage 2 — Host fan-out (needs your host box + real controllers)

**Goal:** the host answers up to 4 slots concurrently, each with its own peer connection,
video delivery, and virtual controller — without breaking single-player.

1. `crates/host/src/main.rs`:
   - Replace the single `host: WebRtcHost` + loose locals (`attached_player_epoch`,
     `force_idr` is fine to keep shared — see below, `last_pad_kind`) with a
     `slots: HashMap<u8, PlayerConn>` where
     ```rust
     struct PlayerConn {
         host: webrtc_peer::WebRtcHost,
         offer_epoch: Arc<AtomicU64>,
         attached_player_epoch: u64,
         last_pad_kind: Option<String>,
         pad: Arc<Mutex<VirtualPad>>,
     }
     ```
   - On `PeerJoined { epoch, slot, .. }`: create (or rebuild, for a reload) the
     `PlayerConn` for that slot only — every other slot's peer is untouched. This
     reuses the existing rebuild-on-rejoin logic in the current `PeerJoined` handler,
     scoped to one entry in the map instead of the one global `host`.
   - `Answer` / `IceCandidate` / `RequestOffer` / `PadInfo` / `PresentPath`: look up
     `slots.get_mut(&msg_slot)` and operate on that entry only, instead of the single
     `host`.
   - Capture/encode stays **shared** — one capture, one encoder, one H.264 bitstream.
     Only the push fans out. Replace both `push_bounded(&host, nal, ...)` call sites
     with a helper that pushes to every currently-connected slot **concurrently**
     (`futures_util::future::join_all` — already a dependency, no new crate needed).
     Sequential awaits here would be wrong: `push_bounded`'s own budget is up to 50ms
     per peer, and the cadence tick can be as tight as 2ms on the pre-encoded path —
     four sequential 50ms awaits would stall the whole capture loop.
   - `force_idr`: keep as one shared flag. An IDR is decodable by every viewer, so a
     freshly-joined slot 3 requesting one costs already-connected slots a harmless
     extra keyframe, not a correctness problem — no need for a per-slot IDR flag.
   - Frame stats (`fps`, `dropped_frames`, etc.) become aggregate across slots — sum
     drops and sends across the concurrent per-slot pushes for the same window. This
     is a simplification worth flagging in review: a genuinely useful version would
     break stats out per slot, which is more UI work than this stage needs to unblock
     4-player.

2. `crates/host/src/emulator_pad.rs`:
   - `apply(kind, id)` → `apply(kind, id, slot)`, threading the slot through to
     `scripts/link-emulator-pad.sh` so it can target the right emulator port. **The
     actual port-mapping logic in that shell script is emulator-specific and untested
     from here** — this is the part that must be verified against your real RPCS3/
     PCSX2 config, resolving the P1-vs-4-ports question above first.

3. `crates/host/src/webrtc_peer.rs::create_virtual_pad`:
   - Already takes no player-identifying param; call it once per slot at
     `PlayerConn` creation time so each slot gets its own `VirtualPad` instance
     (the function signature already supports this — no change needed there).

**Stage 2 exit criteria:** `cargo build --workspace` clean, single-player smoke-tested
by you (existing behavior unchanged when only slot 1 connects), then two-player and
four-player joins tested live with real browsers/controllers on your host.

## Stage 3 — Client display ("show the 4 players")

1. **Web** (`web/src/App.tsx`): handle the new `PlayersStatus` message, show
   "N/4 players connected" in the header pill area (same place the connection-state
   pill already lives). Small, additive change — no new component needed.
2. **Native** (`crates/client/src/*`): same `PlayersStatus` handling in whatever
   currently prints connection state to the terminal/view.
3. **Host console**: log a line on every `PlayersStatus` change
   (`info!("players: {occupied}/{max}")`) — cheap and immediately useful for the
   person running the host during a session.

**Stage 3 exit criteria:** joining as a 2nd/3rd/4th player visibly updates the count on
every connected client and in the host's own log, in your live test.

## Suggested execution order

1. Stage 1 now (safe, self-contained, unit-tested, can land on `main` independently).
2. Stage 2 on a feature branch, tested live against your host before merge — this is
   the one that can break a running session if rushed.
3. Stage 3 alongside or right after Stage 2, since `PlayersStatus` is defined in Stage 1
   but has nothing to display until Stage 2 exists.

## Open questions to resolve before Stage 2 starts

- Does the host's own physical pad occupy one of the emulator's 4 controller ports, or
  a separate one? Determines whether `MAX_PLAYERS` should really be 4 or 3.
- Does `couchlink-ds-vhid` (the Windows companion) support presenting 4 simultaneous
  virtual controllers, or only one today? If only one, Stage 2's per-slot `VirtualPad`
  creation needs a companion-side change too, which is outside this repo's Rust code.
