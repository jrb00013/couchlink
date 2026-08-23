# Plan: stop the pad_info re-apply storm spawning Windows Terminal tabs

**Date:** 2026-08-23
**Symptom:** with friends seated and inputting, the host's Windows Terminal
starts switching tabs on its own and eventually closes — worst while the host
is typing in a terminal.
**Status:** plan only — nothing implemented.

---

## Diagnosis (evidence-backed, not a guess)

Nothing translates gamepad input into keystrokes. No Steam involved. The
keystroke-like behaviour is **console windows being spawned**, and each one
landing as a tab inside the host's Windows Terminal.

### The chain

1. **The web client re-announces `pad_info` constantly.** It is a heartbeat,
   not a one-shot: `PAD_INFO_HEARTBEAT_MS = 3000` (`web/src/player.ts:162`),
   re-sent every 3s per seated player, and *immediately* whenever the reported
   identity changes.

2. **The reported kind flaps.** Three different branches announce different
   kinds for the same player: gamepad → `controllerKind(gp.id)`
   (`player.ts:997`), keyboard fallback → `"generic"` (`player.ts:972`), touch
   → `"generic"` (`player.ts:941`). The Gamepad API drops a pad out of
   `getGamepads()` when it goes quiet, so a player alternating pad and
   keyboard — or just pausing — flips the announced kind back and forth.

3. **The host re-runs the whole binding on every flip.** `SignalMessage::PadInfo`
   is deduped on the raw kind string only
   (`crates/host/src/main.rs:766-776`): `if conn.last_pad_kind != kind` →
   `emulator_pad::apply`.

4. **`apply` shells out to two scripts** (`crates/host/src/emulator_pad.rs:151-162`):
   `ensure-ds-vhid.sh` then `link-emulator-pad.sh`. Between them they invoke
   roughly **12-15 Windows binaries** — `powershell.exe` ×7 for the probe,
   build, local copy, firewall rule and `Start-Process`
   (`ensure-ds-vhid.sh:48,111,113,139,154,156`), plus `tasklist.exe` ×3 and
   `taskkill.exe` ×2 (`:85,95,96,106,120`), plus `powershell.exe` ×3-4 in
   `link-emulator-pad.sh` (`:54,142,509,531`).

5. **Every one of those is a console process, and consoles become tabs.**
   `HKCU\Console\%%Startup` is **unset on this machine** — verified today, the
   key returns nothing. That is exactly the condition
   `docs/INCIDENT-2026-08-19-terminals-died.md` identified: with Default
   Terminal Application unset on Windows 11, every console process spawned
   from WSL attaches into the user's one interactive Windows Terminal. Tab
   opens, steals focus, process exits, tab closes. `install.sh:370` runs
   `scripts/windows/fix-default-terminal.ps1` to repoint this at `conhost.exe`
   — **it is evidently not in effect here** (fresh clone, reinstall, or a
   Windows update reset it).

### The measured scale

`.run/host-restart-cf-1787197040.log` — a **single-player** session — logged
**18** `player pad is …` re-applies, flapping
`generic (keyboard+mouse)` ↔ `xbox (Xbox One Game Controller)` ↔
`generic (touch)`. At ~12-15 console spawns each that is **200+ tab
open/close events in one session, from one player**. Three friends inputting
multiplies it.

### The kicker: every one of those re-applies was pointless

Every line in that log ends `virtual pad backend xbox360`. `backend_for()`
deliberately collapses *every* kind to `xbox360`
(`emulator_pad.rs` + its own regression test
`every_known_pad_kind_uses_the_pcsx2_compatible_backend`). So the thing the
re-apply exists to change **never changed**. The host is deduping on a string
that varies while the value it actually derives from it is constant.

### Second bug found on the way

`ensure-ds-vhid.sh:106,120` `taskkill /F couchlink-ds-vhid.exe`
**unconditionally** before relaunching. The companion is a single process
serving *all three* slots (`SlotRegistry::preallocate`,
`crates/ds-vhid/src/session.rs:80`). So one player's cosmetic kind flap kills
and re-plugs the virtual pads of the other two mid-game. That is very likely
the "my pad randomly dropped" class of report, and it is the same root cause.

### Superseded

An earlier draft of this plan proposed foreground-gating the virtual pads
against Steam Input desktop-mode translation. That hypothesis is dead — no
Steam on this host, and the mechanism above explains the symptom fully. Do
not implement the gate for this bug.

---

## Fix, in priority order

### 1. Dedup on the resolved backend, not the announced kind *(the actual fix)*

`crates/host/src/main.rs` + `emulator_pad.rs`: replace `conn.last_pad_kind`
with `conn.last_backend: Option<&'static str>`, computed via `backend_for(&kind)`,
and re-apply only when *that* changes. Since `backend_for` returns `xbox360`
for everything today, this collapses 18 re-applies to **1**. Keep logging the
kind at `debug` so identity changes stay visible without triggering work.

### 2. Rate-limit and coalesce re-applies per slot *(defence in depth)*

Even with #1, a future `backend_for` that returns more than one value must not
be able to storm. Add a per-slot `last_applied_at: Instant` and a floor of
30s; drop (don't queue) anything inside the window. Also guard against
concurrent `apply` for the same slot with a per-slot flag — today two flips in
quick succession can run both scripts twice in parallel via
`spawn_blocking`, racing the taskkill against the relaunch.

### 3. Make `ensure-ds-vhid.sh` idempotent instead of kill-and-restart

If a companion is already running **with the requested backend**, no-op and
return success. Only kill when the backend genuinely differs. Record the
running backend somewhere cheap (a `.run/ds-vhid.backend` stamp file written
at launch, checked before the taskkill at `:106`). This is what stops one
player's flap from dropping the other two players' pads.

### 4. Re-apply the default-terminal fix, and verify it stuck

`%%Startup` is unset right now. Run `scripts/windows/fix-default-terminal.ps1`.
Then make it self-healing rather than install-time-only: have
`scripts/start-host.sh` check the key and re-apply if unset (the script is
already documented as idempotent and cheap). An install-time-only fix silently
regresses on Windows updates and fresh clones — which is what happened here.

### 5. Don't put couchlink's consoles in a terminal at all *(belt and braces)*

Independent of the registry: the `powershell.exe` calls that already pass
`-WindowStyle Hidden` still create a console. Spawning them detached
(`CREATE_NO_WINDOW`) via a small wrapper, or consolidating
`ensure-ds-vhid.sh`'s seven `powershell.exe` round-trips into **one** script
invocation, cuts the spawn count by an order of magnitude regardless of the
registry state. The consolidation is worth doing on its own merits — it is
also most of the latency in `apply`.

### 6. Client-side: debounce the identity flap

`web/src/player.ts` — when the gamepad disappears from `getGamepads()`, don't
immediately fall back to announcing `"generic"`. Hold the last announced
identity for ~2s before switching. Fixes the noise at the source, and makes
the `player_pad_info` heartbeat stop lying about what everyone is holding.

---

## Files touched

| File | Change |
|---|---|
| `crates/host/src/main.rs` | dedup on backend; per-slot rate limit + in-flight guard |
| `crates/host/src/emulator_pad.rs` | expose `backend_for` for dedup; demote kind logging |
| `scripts/ensure-ds-vhid.sh` | idempotent no-op when backend matches; consolidate powershell round-trips |
| `scripts/start-host.sh` | verify/re-apply `fix-default-terminal.ps1` |
| `web/src/player.ts` | debounce gamepad-disappearance identity flap |

---

## Tests

Unit (`crates/host`):
1. Same backend announced twice → `apply` runs **once**.
2. `generic` → `xbox` → `generic` (the exact logged flap) → `apply` runs
   **once**, because all three resolve to `xbox360`. Direct regression on the
   18-re-apply log.
3. Two flips inside the rate-limit window → one apply, second dropped.
4. Genuine backend change after the window → applies.

Script:
5. `ensure-ds-vhid.sh` with a healthy companion already on the requested
   backend → exits 0 **without** calling `taskkill`.

Manual:
6. Three friends seated and inputting, host typing in a terminal → zero tab
   switches, zero tab closes.
7. Count `player pad is` lines in the host log across a full session → should
   be one per seated player, not 18.
8. One player switches pad→keyboard mid-game → the other two players' pads
   never drop.
