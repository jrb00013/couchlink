# Split-Screen Isolation (per-player view crop) — Implementation Plan

**Goal:** An opt-in toggle in the player UI so a player sees **only their own split-screen
region** instead of the full 2/4-way layout. Purely a *view* concern — pad routing, host
encode, and the wire are untouched.

**Status (2026-10-04):** Design only. Nothing implemented. Issue: #85.

**Driving use case:** local split-screen via the friend's ViGEm pad as P2 — see
`contrib/rpcs3-bo2-splitscreen/README.md` and
`docs/superpowers/plans/2026-09-29-bo2-splitscreen-livelock.md`. BO2 4-player is a
2×2 grid; the layout cases below are chosen from it plus the usual horizontal/vertical
2-player splits.

## Hard constraints (from the request)

- MUST NOT add host overhead — not "negligible", **zero new lines on the encode thread**.
- MUST NOT add stream latency.
- MUST NOT add input response delay.
- MUST fit the existing architecture (host captures+encodes once → all slots get the same track).

## Known facts (verified in this repo, not assumed)

- Both web present paths funnel through exactly one draw call each:
  `web/src/lowLatencyCanvas.ts:218` and `web/src/webCodecsCanvas.ts:600`, both
  `ctx.drawImage(frame, 0, 0)`. The frame is already decoded at that point.
- The host encodes **once** and fans the same NAL to every peer
  (`crates/host/src/encode.rs`, `push_to_all` in `crates/host/src/main.rs`).
  `StreamInfo` is a single broadcast. Nothing per-player exists on the encode path.
- `PadFrame` rides `PAD_CHANNEL`, a separate stream from video. No isolation config is
  readable from the pad path ⇒ input latency is causally downstream of nothing.
- `crates/client/src/view.rs:112` hardcodes `uvs` to `0..1`; `view.rs:132` samples
  `in.uv`. `@group(0) @binding(3)` at `:125` is the highest binding ⇒ `binding(4)` is free.
- `crates/proto/src/host_events.rs:7` declares `HostToPlayer`, documented as "optional
  host→player control messages (JSON) on the pad channel" — **zero consumers in the tree.**
- **`crates/signaling/src/ws.rs` dispatches relays by explicit per-variant `match` arms and ends
  in `_ => {}` — an unrecognised variant is SILENTLY DROPPED.** Any new message that is not
  added there by hand never reaches the host or the players, with no error anywhere.
- Precedents for each direction we need already exist in that match: `PadInfo` / `PresentPath` /
  `ClientLinkStats` for *player → host with the server stamping the slot*, and
  `StreamInfo | HostStats` for *host → broadcast to all players* (`ws.rs:322`).
- The server stamps the **connection's own registered slot**, never the client-supplied one:
  "a client-supplied slot is never trusted, so one player can't spoof messages for another
  player's slot" (`ws.rs:9-11`, `ws.rs:224`). Slot assignment itself is authoritative from the
  session's `PlayerTable` (`crates/signaling/src/players.rs`), conveyed in `Registered`
  (`crates/proto/src/signal.rs:23-29`).
- `web/src/seat.ts` `seatForRemoteSlot()` maps remote slot → seat (host = P1, remotes = P2–P4).
- `lowLatencyCanvas.ts:108` sets `imageSmoothingEnabled = false` (the nearest-neighbor
  1206≠1280 stutter fix). `webCodecsCanvas.ts:252` sets it `true`.
- No existing crop/viewport/isolation code anywhere in `web/src` or `crates` — clean slate.

## Why the constraints are satisfied by construction

The crop is the 9-arg `drawImage(frame, sx, sy, sw, sh, dx, dy, dw, dh)` source-rect overload:
no second decode, no new texture, no re-encode, no network change. NDC positions in `view.rs`
stay fullscreen, so the sub-rect simply upscales to fill the window. So the design questions are
only: **(a) how do we learn the rect once, (b) how do we guarantee the first painted frame is
already cropped.**

## Task 1: Protocol — intent over the signaling WS

Add to `SignalMessage` in `crates/proto/src/signal.rs`, mirroring in `web/src/proto.ts`:

| Variant | Direction | Payload |
|---|---|---|
| `split_screen_set` | player → host | `enabled`, `layout`, `regions[]`, `seat_region{}`, `manual` |
| `split_screen_config` | host → all | same + authoritative `enabled` |

- New `Rect { x, y, w, h }` as `f32`, all fields `#[serde(default)]`, clamped to `[0,1]`.
- **Rects are normalized, not pixels.** The host changes resolution mid-session
  (`set_target`, `target_width/height` in `host_stats`); normalized rects survive that with
  zero re-detection. Pixel rects would go stale silently.
- Route over the **WS, not the pad DataChannel.** The WS is live from registration; the pad DC
  may not exist yet if a player arms isolation before WebRTC finishes negotiating.
- The server stamps the connection's **own** slot and ignores any client-supplied slot
  (`ws.rs:9-11`), so one player cannot request isolation on another's behalf — this is what makes
  routing intent through the WS safe, and it is free.
- Do **not** reuse `host_events::HostToPlayer` — it is pad-channel JSON and unused; leave it.

## Task 2: Wire the relay — *without this the feature is dead on arrival*

`crates/signaling/src/ws.rs` relays by explicit per-variant `match` arms and ends in `_ => {}`.
A new variant that is not added here is **silently dropped**, with no error on either side. Two
arms, both mirroring existing precedent:

- `SplitScreenSet` → *player → host, stamp the connection's own slot*, exactly like `PadInfo`
  (`ws.rs:255`) and `PresentPath` (`ws.rs:275`):
  `relay_to_host(&store, sid, &SignalMessage::SplitScreenSet { …, slot: my_slot })`.
- `SplitScreenConfig` → *host → broadcast to all players*, add to the existing
  `StreamInfo | HostStats { .. }` arm (`ws.rs:322`) so it hits `store.broadcast_to_players`.

Verification for this task specifically: with the proto variant added and the arm **not**, a
player toggling isolation must change nothing and log nothing. With the arm, the host must log
the config. If the toggle appears to do nothing at all, this task is the first thing to check.

## Task 3: Detector — one-shot, client-side, in a Worker

New `web/src/splitDetector.ts` + `web/src/splitDetector.worker.ts`.

**Why the client and not the host:** the host is the most CPU-constrained machine in the
session (emulator + encoder + link governor + photon budget all resident). "Host executes zero
lines of this code" is a categorically stronger guarantee than "host overhead is small."

**Why never per-frame:** `getImageData` is a synchronous GPU readback. On the rAF paint loop it
stalls the compositor and blows the latency budget this codebase is built around. One-shot per
toggle, on a 160×90 downscale, transferred to a Worker.

Algorithm:

1. `drawImage(frame, 0, 0, 160, 90)` into an `OffscreenCanvas` → `getImageData` **once**.
2. Build column-mean luma `L[x]` and row-mean luma `L[y]`.
3. For each candidate line in the central 60%, compute mean `|L[n] − L[n−1]|` over the
   **central 80% of the perpendicular axis only** — HUD bars live at the top/bottom and would
   otherwise dominate the gradient profile.
4. Score each candidate against the **median gradient of that line**. `argmin` absolute gradient
   finds noise; `argmin` relative to the line's own median finds a genuine seam.
5. Higher-scoring axis wins the layout (horizontal vs vertical).
6. From the minimum, **expand outward** while gradient stays under a low threshold — absorbs the
   2–4px black gutter real games draw between halves.
7. A second seam on the perpendicular axis ⇒ 2×2 grid.
8. Best seam below the score floor ⇒ `no-split-found`. Fall back to full frame + manual prompt.
   **A wrong crop is worse than no crop.**

Deterministic ⇒ every client agrees from the same source pixels, so **rects need not cross the
wire**. The host carries intent only — enough for the roster UI and to arbitrate when a client
fails detection.

## Task 4: Web renderers — crop + paint gate

Both `web/src/lowLatencyCanvas.ts` and `web/src/webCodecsCanvas.ts`:

- Add `cropRect: SrcRect | null` and a `setCrop(rect | null)` to the view.
- In `paint()`, size the canvas to the **crop** (not the frame) and use the 9-arg `drawImage`.
  This is the behavioural change at `lowLatencyCanvas.ts:212` / `webCodecsCanvas.ts:594`.
- `cropRect === null` ⇒ emit today's exact `ctx.drawImage(frame, 0, 0)`. Disabled behaviour must
  be byte-identical.
- **Paint gate (the correctness bug):** while isolation is armed and `cropRect` is null, clear
  to black and never `drawImage` uncropped. Otherwise the first painted frame shows the entire
  screen — exactly what we are hiding — for a few hundred ms on every join.
- Preserve `imageSmoothingEnabled` as-is per path; do not "fix" the blockiness by flipping it.

## Task 5: Native client — UV crop uniform

`crates/client/src/view.rs`:

- Add `var<uniform> crop: vec4<f32>` at `@group(0) @binding(4)` (free — `:125` is the last).
- `out.uv = crop.xy + uv * crop.zw`. NDC positions unchanged ⇒ sub-rect upscales to fill.
- `set_crop(x, y, w, h)` / clear; same paint gate (solid black) when armed-without-rect.
- Thread the config from signaling handling in `crates/client/src/main.rs` to the view.
- No decode, texture, or vertex change.

## Task 6: UI + toggle

`web/src/App.tsx`:

- Toggle in the roster/controls area (the roster at `App.tsx:946` already renders one tile per seat).
- On ON: one-shot detection from the current video frame → send `split_screen_set` → apply locally.
- Status line: "Auto-detected: horizontal (P2 right)" / "Manual" / "Not detected".
- Persist to `localStorage['couchlink.split:' + sessionId + ':' + W + 'x' + H]` so a returning
  player crops **from frame one** with no detection at all.
- Key region arrays off **seat** (via `seatForRemoteSlot`), not slot, so all four players index
  identically.
- Manual rect override with an optional bleed — the escape hatch for titles that draw a shared
  full-width HUD strip across the seam.
- Default binding: 2p horizontal → P1 left / P2 right; vertical → P1 top / P2 bottom; 2×2 →
  row-major. Overridable.

## Task 7: Pluggable detector (deferred, off by default)

`SplitDetector` trait; `HeuristicSplitDetector` is the only shipped implementation.
`ModelSplitDetector` behind `COUCHLINK_SPLIT_MODEL=1` requiring an **explicit local model path**,
never auto-download.

Feature-flag the interface, not the payload: a few MB of model competing with `VideoDecoder` for
the same CPU, fetched over the same connection carrying the game, is a bad trade no matter how
one-shot the inference is. Heuristic-first is the correct default.

## Task 8: Verify

- [ ] 2p horizontal: each player sees only their half; the other's area is never visible.
- [ ] 2×2 grid (BO2 4-player) maps row-major to P1–P4.
- [ ] Toggle OFF ⇒ full frame, byte-identical to today's `drawImage(frame, 0, 0)`.
- [ ] Arm before frames arrive ⇒ black until config known, **never** a full-screen flash.
- [ ] Returning player crops from frame one (persisted config).
- [ ] Detection failure ⇒ full frame + manual prompt, no crash.
- [ ] Host CPU/GPU flat vs baseline across a 10-min 4p session (`host_stats` before/after).
- [ ] Latency + input parity: `__couchlinkRicardo` photon/`input_freshness` unchanged vs baseline.
- [ ] Old peer joins a new session ⇒ ignores the new messages, session still works.
- [ ] **Relay arm exists** — toggle with the arm removed changes nothing and logs nothing (see Task 2).
- [ ] Native client crops identically.

## Known costs (accepted, stated up front)

1. **The crop is free; the upscale is not.** 2p horizontal at 1920×1080 ⇒ each player gets
   960×540 stretched to their display, and the RTP path has smoothing **off** ⇒ visibly blocky.
   Mitigation: raise the capture preset for split sessions via the existing `set_target`, or
   accept the softness. **Users will notice this one, not the crop.**
2. **Bandwidth is unchanged.** Every isolated player still receives the full frame; a 4p
   all-isolated session ships 4× the pixels nobody needs. At 1080p60 on a home connection that is
   clearly worth it for zero host cost and zero added latency. If it ever isn't, per-peer
   `set_target` scaling already exists.
3. **Visual crop is not secrecy.** The full frame is in client memory and trivially recoverable.
   Fine for couch co-op, categorically insufficient for competitive play. The same
   `split_screen_config` message is where a per-peer encode policy would be signalled later.
4. **Shared full-width HUD strips get cut in half** on horizontal-split titles. Manual override
   is the escape hatch; label the feature experimental for those games.

## Review Focus

- **Paint gate is mandatory on both paths.** Arm-without-config rendering an uncropped frame is
  the one bug that defeats the entire feature. Check `view.rs` as carefully as the web path.
- **Seat→region binding direction.** A backwards binding means you watch your opponent while
  controlling your own character — reads as a game bug, not a settings mistake. Assert the
  mapping in a unit test, don't eyeball it.
- **Nothing may be added to the paint loop** — no `getImageData`, no layout thrash, no
  reallocation when `cropRect` is unchanged. Check the diff for per-frame work.
- **Disabled path must be byte-identical.** Guard the crop behind `cropRect === null` rather
  than branching early-return, so the default path cannot drift.
- **Normalized rects only.** Any pixel-space rect is a latent bug the next time `set_target`
  fires.
