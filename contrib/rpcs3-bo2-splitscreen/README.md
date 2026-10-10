# RPCS3 Black Ops II (BLUS31011) split-screen pack

## Problem

Local split-screen (the mode couchlink needs when a friend's ViGEm pad is P2)
livelocks BO2's main thread under doubled SPURS load. Treyarch's `hang detect`
thread then deliberately writes to address `0x17` and kills the process
([RPCS3#16426](https://github.com/RPCS3/rpcs3/issues/16426)).

## What this pack does

1. **Game patch** (`BLUS31011_patch.yml`) — for PPU hash
   `PPU-fa8ffe9ae59bfa8575027c9a6ec5aab7fb0215ab` (loads ~26s into boot / when
   multiplayer code maps in):
   - Force watchdog enable-flag read to 0 (`0x0041c3cc`: `lbz` → `li r3,0`)
   - NOP the intentional crash store (`0x0041c414`: `stw` → `nop`)
2. **Per-game config** — **Multithreaded RSX off**, **Frame limit Auto**,
   permanently for BLUS31011. Also: Accurate SPU DMA, PPU reservation priority
   over SPUs, Safe SPU block size, Max SPURS Threads 4, no exclusive fullscreen
   (couchlink WGC capture).

   Stage the **couchlink-play** fork binary (`switch-rpcs3.cmd couchlink-play`)
   so Vulkan CB reclaim/wait + SPURS/HID fixes are present — that stops
   *present death* (`CB chain has run out of free entries`). It does **not**
   make MT RSX safe: with MT on, BO2 still softlocks in SPURS wait loops
   (stuck FPS, runaway `sys_timer_usleep`), especially opening the in-game
   settings menu. Keep MT RSX **false** even after couchlink-play is staged.

This does **not** claim to reverse-engineer Treyarch's full job system. It
removes the suicide and breaks the infinite SPURS wait so split-screen can
be *played*. Remaining glitches from early-exit waits are preferred over an
unrecoverable softlock.

## Softlock patch (what it changes)

`main_thread` wait at `0x73d690` normally:

1. `ld` completion counter @ `r30`
2. exit if done (`*r30 <= r31` via `cmpld` / `ble`)
3. `bl 0x73d09c` (try to progress jobs)
4. `sys_timer_usleep(50)`
5. unconditional `b` back to step 1

When the counter never completes, step 5 livelocks (stuck FPS, runaway
usleep) — common under 4-pad / split-screen SPURS load. **Invariant the game
already trusts:** wait is done iff `*r30 <= r31`. Patch replaces steps 4–5
with `std r31,0(r30)` then branch to epilogue — one progress attempt, then
force the completion word and return. Callers/Backend see a consistent done
state (v1 one-poll-without-store caused Backend AV at `0x16`). Enabled as
`BO2 softlock: force SPURS wait complete then exit (couchlink)`.

## Install

```bash
RPCS3_DIR=/mnt/c/Users/YOU/RPCS3 ./install.sh
```

Then boot BLUS31011. Confirm in RPCS3: **Manage → Game Patches** that
`BO2 split-screen: disable hang-detect watchdog (couchlink)` is checked.

## Proof protocol

1. Host boots BO2 with this pack; couchlink capture window needle `31011`.
2. Friend joins; both pads present; start split-screen Zombies (or campaign co-op).
3. Play ≥10 minutes.
4. **Pass:** no `Access violation writing location 0x17`, and no stuck-FPS
   softlock after settings / grenade / split-screen spikes (softlock patch).
5. If a soft stall still recovers slowly, capture `RPCS3.log` + Kernel Explorer;
   next target is identifying the missing SPURS producer (see `tools/bo2-re/`).
