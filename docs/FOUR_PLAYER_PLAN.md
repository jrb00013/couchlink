# 4-player support — implementation plan

Status: **Stages 1–3 built**. Stage 1 (signaling layer: slots on the wire, `PlayerTable`
wired into `Session`, slot-aware relay, `PlayersStatus` broadcasts), Stage 2 (host fan-out:
per-slot `PlayerConn`, concurrent `push_to_all` fan-out, shared encoder + single bitrate
governor), and Stage 3 (web "N/3 players" pill, native client + host console logs) are all
implemented. `cargo test --workspace` is green (164 tests) and the web build/tests pass
(`tsc --noEmit` + 62 vitest tests). Everything is uncommitted on `main`. Live multi-player
verification against the real host box, real controllers, and a real emulator is still
outstanding (see Open questions).

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
| `crates/signaling/src/players.rs` — `PlayerTable`, 3 slots, tested | Built, **wired in** (Stage 1) |
| `crates/signaling/src/session.rs` — `Session.players: PlayerTable` | Slot table replaces the single-slot store (Stage 1) |
| `crates/signaling/src/ws.rs` — relay | Slot-stamped, slot-routed, `PlayersStatus` broadcasts (Stage 1) |
| `crates/proto/src/signal.rs` — wire protocol | `slot` fields + `PlayersStatus` message (Stage 1) |
| `crates/host/src/main.rs` / `webrtc_peer.rs` | `slots: HashMap<u8, PlayerConn>`, per-slot peers, concurrent fan-out (Stage 2 built) |
| `crates/host/src/emulator_pad.rs` | `apply(kind, id, slot)` binds per-slot to emulator P2–P4; `run()` threads `COUCHLINK_EMU_PLAYER` (Stage 2 built) |
| Web / native clients | Web shows "N/3 players connected" pill; native + host log `players: N/3` (Stage 3 built) |

**Resolved:** the host's own physical pad owns emulator P1, and remote slots 1–3 map to
P2–P4 — `MAX_PLAYERS = 3` fills exactly the 4 ports RPCS3/PCSX2 expose. A fourth remote
join is rejected with "session full (3/3)".

## Stage 1 — Signaling layer (safe, no hardware, fully unit-testable)

**Goal:** up to 3 players can hold a slot simultaneously without evicting each other
(the host's pad owns emulator P1, so remote slots 1–3 fill P2–P4).

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
    - `register_player` calls `players.assign(tx)`; returns `Err("session full (3/3)")`
      when `PlayerTable::assign` returns `None`.
   - `unregister(session_id, Role::Player, tx)` needs the slot to vacate — change its
     signature (or add a `player_slot_of(tx)` lookup) so only the socket that owns a
     slot can vacate it, mirroring the existing stale-socket-can't-evict tests.
   - Add `player_tx(session_id, slot) -> Option<WsSender>` alongside the existing
     `peer_tx`.
   - Update/extend the existing tests in this file's `mod tests` (`player_rejoin_...`,
     `a_stale_player_socket_closing_does_not_evict_its_replacement`) for the new shape,
      and add: `a_fourth_player_is_rejected_when_the_table_is_full`,
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
          host: Arc<webrtc_peer::WebRtcHost>,
          attached_player_epoch: u64,
          last_pad_kind: Option<String>,
          pad_feedback_task: Option<tokio::task::JoinHandle<()>>,
      }
      ```
      **Built as:** the plan's `offer_epoch` and `pad` fields were dropped — dead code.
      `offer_epoch` / `player_slot` / the pad are created inside `build_player_conn` and
      cloned into the `WebRtcHost` (which keeps its own Arcs); the pad is only referenced
      by the spawned per-slot feedback task. Each slot's `VirtualPad` is a separate
      controller on the emulator's P2–P4 port, with its own rumble/adaptive-trigger
      feedback loop aborted on leave/rebuild (`close_conn` spawns `pc.close()` with a 5s
      timeout so it can't hang the media loop).
    - On `PeerJoined { epoch, slot, .. }`: create (or rebuild, for a reload) the
      `PlayerConn` for that slot only — every other slot's peer is untouched. This
      reuses the existing rebuild-on-rejoin logic in the current `PeerJoined` handler,
      scoped to one entry in the map instead of the one global `host`. A rejoin burst is
      coalesced per slot (`take_queued_joins`) and non-join messages that arrived during
      the drain are replayed (`route_slot_msg`), so a reload can't interleave with a
      fresh answer.
    - `Answer` / `IceCandidate` / `RequestOffer` / `PadInfo` / `PresentPath`: look up
      `slots.get_mut(&msg_slot)` and operate on that entry only, instead of the single
      `host`. The pre-slot `PeerLeft` broadcast (slot 0) drops a lone connection and
      refuses to guess when more than one is seated.
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
    - **One bitrate governor, not one per slot** (applied-math §2). `LinkGov` commands a
      single shared knob (`capturer.set_target()` — the Windows encoder has one output).
      N per-slot governors would fight over that knob — the last writer wins, and a
      struggling slot and a comfortable slot oscillate the shared encoder between rungs.
      The governor must take the whole vector of per-slot congestion signals and pick
      one shared target (e.g. the lowest rung any connected slot needs).
    - **The uplink is the hard bound** (applied-math §1). `b1+b2+b3 ≤ B_uplink` does not
      grow with player count: 720p60 ≈ 10 Mbps, 1080p60 ≈ 18 Mbps per peer, so 3 peers
      at today's per-peer target needs ~30–54 Mbps of consistent uplink. Verify the host
      box has that headroom before calling 4-player done, and treat the shared governor
      as the thing that keeps a full table inside it.

3. `crates/host/src/webrtc_peer.rs::create_virtual_pad`:
   - Already takes no player-identifying param; call it once per slot at
     `PlayerConn` creation time so each slot gets its own `VirtualPad` instance
     (the function signature already supports this — no change needed there).

4. **Simulcast tiers instead of one shared bitrate** (applied-math §3) — follow-up to
   the shared governor, not required for the first 4-player build:
   - Encode 2 fixed tiers once each (e.g. 720p60 + 480p30) — still bounded work (2
     encodes, not N) — and let each slot's answer/negotiation pick the tier its own
     measured headroom supports. A single shared target otherwise drags a LAN player
     down to whatever the weakest peer needs.
   - Start at 2 tiers and measure before adding a third.
   - This is the correct long-term answer to "not everyone has 30 Mbps of uplink";
     defer it if Stage 2 is already large.

**Not optimizing:** the pad/input path (applied-math §4). PadFrame is bytes, not video —
N independent pad channels cost nothing measurable against a ~10 Mbps video budget.
Leave it alone.

**Longer-term, if more players than the uplink supports:** push fan-out off the host
(applied-math §5) — the host is effectively its own SFU today, paying the N-multiplication
out of its own uplink. If TURN relaying is in play, ask whether the relay can duplicate
server-side (host sends one stream, relay fans out to N peers) instead of N full-bitrate
TURN allocations. Bigger than Stage 2 needs; decide before investing in N-player growth.

**Stage 2 exit criteria:** `cargo build --workspace` clean (met), single-player smoke-tested
by you (existing behavior unchanged when only slot 1 connects), then two-player and
four-player joins tested live with real browsers/controllers on your host.

## Stage 3 — Client display ("show the 4 players") — built

1. **Web** (`web/src/App.tsx` + `web/src/player.ts` + `web/src/proto.ts`): `players_status`
   added to the web `SignalMessage` union; `onPlayersStatus` callback threaded through
   `player.ts` → `usePlayerCallbacks.ts` → `App.tsx`, which shows "N/3 players connected"
   in a header pill next to the connection-state pill (only after the first status
   broadcast, so it never flashes on the pre-join screen).
2. **Native** (`crates/client/src/main.rs`): handles `PlayersStatus` with
   `info!("players: {occupied}/{max} connected")`.
3. **Host console**: `info!("players: {occupied}/{max}")` on every received status.

**Stage 3 exit criteria:** joining as a 2nd/3rd/4th player visibly updates the count on
every connected client and in the host's own log, in your live test.

## Suggested execution order

1. Stage 1 now (safe, self-contained, unit-tested, can land on `main` independently). — **Done**
2. Stage 2 on a feature branch, tested live against your host before merge — this is
   the one that can break a running session if rushed. — **Implemented; live test pending**
3. Stage 3 alongside or right after Stage 2, since `PlayersStatus` is defined in Stage 1
   but has nothing to display until Stage 2 exists. — **Done**

## Open questions to resolve before Stage 2 starts

- ~~Does the host's own physical pad occupy one of the emulator's 4 controller ports?~~
  **Resolved: yes.** The host owns P1; `MAX_PLAYERS = 3` remote slots fill P2–P4.
- **Still open — live verification needed:** does `couchlink-ds-vhid` (the Windows
  companion) support presenting 4 simultaneous virtual controllers, or only one today?
  `scripts/link-emulator-pad.sh` already threads `COUCHLINK_EMU_PLAYER` and the host sets
  it to `slot+1` per slot; the naming (XInput-0/-1, `Wireless Controller 1/2`) and
  multi-device behavior need a real-box test.
- **Still open — live verification needed:** does the host box's uplink actually have the
  ~30–54 Mbps headroom (3 peers × 10–18 Mbps) that 4-player at today's presets needs?
  If not, the shared governor can hold the total inside a lower budget, but real 4-player
  then needs the simulcast tiers (Stage 2 §4) or a non-trivial quality cut. `push_to_all`
  feeds the single shared governor the *sum* of every slot's sheds for exactly this.
