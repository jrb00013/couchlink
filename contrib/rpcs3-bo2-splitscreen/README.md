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
2. **Per-game config** — crash-safe until the **couchlink-play** fork binary is
   staged (`switch-rpcs3.cmd couchlink-play`): **Multithreaded RSX off**,
   **Frame limit Auto**. That build is the co-play line (SPURS hang fix + HID
   pad lock + Vulkan CB reclaim/wait) so MT RSX + heavy present (split-screen /
   multi-pad / friends) stays up. After it is active, turn **Multithreaded RSX
   on** again for last-night stick/present feel. Also: Accurate SPU DMA, PPU
   reservation priority over SPUs, Safe SPU block size, Max SPURS Threads 4,
   no exclusive fullscreen (couchlink WGC capture).

This does **not** claim to reverse-engineer Treyarch's job system. It removes
the suicide and reduces the race window so split-screen can be *played and
proven*. If a soft stall still happens, the emulator stays alive so Kernel
Explorer / further patches can finish the livelock.

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
4. **Pass:** no `Access violation writing location 0x17`. Soft freezes that
   recover count as pack success (watchdog dead). Hard `0x17` = patch miss.
5. Capture `RPCS3.log` if it soft-stalls without recovering — next patch target
   is the wait loop at `0x73d6ec` in the same PPU module.
