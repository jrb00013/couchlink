# RPCS3 stall: open issues and step-by-step diagnosis plan

Status as of 2026-10-01. Companion to `docs/RPCS3_FREEZES.md` (what happened, tools, runbook).
Evidence levels are marked: **PROVEN** (observed in logs/dumps), **SUSPECTED** (fits the evidence, not tested),
**UNKNOWN**.

Issues live in the fork: https://github.com/jrb00013/rpcs3/issues (#3-#6).

## 1. What is wrong (and what is not)

Co-op sessions on the RPCS3 fork (Windows, Vulkan, 24 host threads) intermittently "freeze": the picture stops or
the game stops responding for 20 s to ~6 min, then recovers by itself. Nothing is logged by the game while it
happens. Nobody has found a hard deadlock; every stall so far ended on its own.

| # | Problem | Evidence | Status |
|---|---|---|---|
| A (issue #3) | **BO2 (BLUS31011) lobby/party stalls**: PPU `Secondary` thread starves in a `lwarx/stwcx.` retry loop while SPURS SPU kernels poll; `main_thread` waits for the counter it decrements | Guest thread dump 2026-10-01 11:23 (`main_thread` `sys_timer_usleep(0x32)` at `LR=0x73d7a8`; `Secondary` at `0x73d5ac`; 6x `CellSpursKernel` ~99% CPU) | Mechanism **SUSPECTED**, observation **PROVEN** |
| B (issue #4) | **MK (BLUS30522) stalls ~0:41 into boot on every 20078-based fork build**, fine on 19984-based | Reproduced on `20078`, `20078-hidfix`, `20078-cbwait`, passes on `19984-fix`/`19984-play*` | Regression window upstream 19984..20078, cause **UNKNOWN** |
| C (issue #5) | **MK story-mode cutscene-end stalls** (118 s and 222 s): movie threads sit in `_sys_ppu_thread_exit`, `main_thread` silent-spins on `sys_timer_usleep`, `ZLibCellSpursKernel0` busy | Two occurrences in the log; both ended on their own | Cause **UNKNOWN** (may be the same class as A) |
| D (issue #6) | **BO2 `SPU Unknown STOP code 0x0`** (SPURS kernel executed zeroed LS at `0x28984`) ~4 min in | One crash, with the SPURS force-complete game patch enabled | Cause **UNKNOWN**; patch now disabled |

Ruled out (with evidence): per-game config drift, HID lock, Vulkan CB-wait, SPURS `send_event` fix, compile-worker
count (19984 also uses 12), shader-interpreter precompile as the sole cause (MK still stalls with it off), audio
device, a vdec "waiting for consumer" error, suspend/resume "nudge" (BO2 stall ended 323 s after one).

Not a stall signal: window-title FPS (kept saying ~60 while MK was frozen), `sys_timer_usleep` rate (healthy MK
17-50k/s vs 62k/s stalled), host CPU (SPURS kernels idle-poll at ~99% even when healthy).
**The signal that works: game-event silence** (no non-audio guest log line for >= 30 s).

## 2. What remains to be fixed

1. **A: confirm or refute the reservation-starvation mechanism, then fix it in the core** (not with game patches).
   Candidate area: `rpcs3/Emu/Cell/PPUThread.cpp` `ppu_store_reservation` / `ppu_load_acquire_reservation`, and
   SPU `GETLLAR/PUTLLC` polling (`SPUThread.cpp`). The `PPU Reservation Priority Over SPUs` option only affects
   `vm::writer_lock` (`vm.cpp:475`), not the fast `stwcx.` path.
2. **B: bisect the upstream range 19984..20078** for the MK boot stall (needs a repro-per-build; each CI build ~50 min).
3. **C: capture a cutscene-end stall with the new dump** (`--guest`, now with PDB) and compare with A.
4. **D: reproduce `STOP 0x0` without the game patch**; capture the full SPU context (MFC queue, tag state).
5. **Tooling gap:** `dump_threads.trigger` dumps only PPU threads. Extend it to SPU threads (PC, LS around PC, MFC
   queue/tag status, last `GETLLAR` address). Needed for step 6 below.
6. **Pack settings are unproven guesses.** `Disable SPU GETLLAR Spin Optimization`, `SPU Wake-Up Delay 20`,
   `Max SPURS Threads 4` (BO2 still runs 6 SPURS kernels), etc. came in one commit with no evidence. Each needs an A/B.

## 2b. Fix candidate in flight (2026-10-01)

Fork `couchlink-play-19984` @ `c96e609` (PR #2): `PPU Reservation Priority Over SPUs` now also covers the PPU
`lwarx/stwcx.` path (SPU back-off when a PPU atomic recently failed on the line). Evidence it targets the right thing:
instrumented-build dump showed ~1M `stwcx.` failures in the first 3 min on a handful of lines, including `0x0269e400`
which CellSpursKernel1 was `PUTLLC`-ing. **Unverified**: needs the A/B in `tools/rpcs3-debug/TEST_NIGHT.md`.
If it works, close issue #3 with the before/after numbers; if not, the histogram says which line to chase next.

## 3. Step-by-step diagnosis plan

Rules: never restart before capturing; one variable per experiment; record every episode; label results
PROVEN/SUSPECTED/UNKNOWN. Tools: `tools/rpcs3-debug/` (`rpcs3-hang-diag.sh`, `rpcs3-stall-watch.sh`,
`threads.ps1`, `analyze-dump.py`, `symbolize.py`).

**Step 0 - Baseline setup (once per build)**
1. Run a build that has the PDB and the dump trigger (fork branch `couchlink-play-19984`, commit >= `7fd007b`).
   Keep `versions/<name>/pdb/rpcs3.pdb` next to the exe.
2. `tools/rpcs3-debug/rpcs3-stall-watch.sh --bg` (auto-captures every episode into `~/rpcs3-stalls/<time>/`).
3. `python3 -m venv ~/.venvs/md && ~/.venvs/md/bin/pip install minidump` for the dump analysis.
4. Record: build id, game, config (`custom_configs/config_<serial>.yml`), pack settings, number of players.

**Step 1 - Detect and classify** (when someone says "frozen")
1. Do not restart. `tools/rpcs3-debug/rpcs3-hang-diag.sh --shot`.
2. Verdict: `LOADING` -> wait; `GUEST_STALL` -> continue; `DEADLOCK` -> go to step 2 immediately; `NOT_RUNNING` -> read
   the fatal lines (`^·F`) and save the log before relaunching (the previous log is archived and unreadable).
3. Note the wall time and what the player was doing (lobby, party, cutscene, loading).

**Step 2 - Capture while stuck** (<= 1 min)
1. `rpcs3-hang-diag.sh --guest` -> `<RPCS3 dir>/log/guest-threads-<time>.txt` (all PPU thread contexts).
2. `rpcs3-hang-diag.sh --dump --shot` (small minidump, ~5 s pause). **Never `--dump` with a full dump** (37 GB, suspends the game).
3. Copy the `RPCS3.log` tail (the dump block) into the episode folder.

**Step 3 - Read the guest dump (what the game is waiting for)**
1. For `main_thread`: `In function`, `LR`, `CIA`, and the disassembly around `LR` (it shows the wait loop and which
   register holds the polled address/counter).
2. List every thread's state: `awk` summary of `State`, `In function`, `Waiting` (see `docs/RPCS3_FREEZES.md`).
   The thread that is **running** (state `00[]`) while everyone else waits is the one that is not making progress.
3. Disassemble its `CIA`/`LR` neighbourhood: `lwarx/stwcx.` retry = reservation contention; syscalls = wait on object.
4. Write down the reservation address (register used with `lwarx`) and the memory it points to.

**Step 4 - Read the native dump (what the emulator threads are doing)**
1. `~/.venvs/md/bin/python tools/rpcs3-debug/analyze-dump.py <dmp> versions/<name>/pdb/rpcs3.pdb`.
2. Expect SPU threads executing in `rpcs3.exe`: resolve names (e.g. page-lock loop, reservation compare).
   Threads in JIT code are LLVM-recompiled guest code. Threads in `ntdll` are waiting.
3. Cross-check with `threads.ps1` CPU table: who is >90% and in which function.

**Step 5 - Decide the class**
- `stwcx.` retry + SPU kernels polling the same 128-byte line -> reservation starvation (A).
- Silent `sys_timer_usleep` spin with no running PPU thread -> a wait on a missing event; find the event owner (SPU job / semaphore).
- SPU STOP/zeroed LS -> job code/data not loaded (DMA/MFC or freed memory): capture MFC queue (needs tooling gap 5).

**Step 6 - Extend the instrumentation (code, log-only)**
1. SPU thread dump on the same trigger (tooling gap 5): `spu_thread::dump_all` for every SPU thread in the group.
2. Add a per-address `stwcx.` failure counter in `ppu_store_reservation` and print the worst offenders on trigger
   (address, fail streak, who holds `r & 127`). Log-only, behind the trigger.
3. Rebuild (CI ~50 min), rerun steps 0-4 on the next stall. Proves or kills the starvation mechanism.

**Step 7 - Controlled experiments (one variable at a time, >= 3 sessions each, same player count)**
Metric: episodes/hour and median duration from `~/rpcs3-stalls/*/result.txt`.
1. Baseline (current: spin optimization on, SPURS patch off).
2. `SPU GETLLAR Busy Waiting Percentage` 100 -> 50.
3. `Max SPURS Threads` actually limiting SPURS kernels (BO2 uses 6; check whether the setting applies).
4. `SPU Wake-Up Delay` 20 -> 0; `PPU Reservation Priority Over SPUs` on/off.
Keep only changes that reduce episodes; record results in the issue.

**Step 8 - Fix in source (only after step 5/6 identifies the mechanism)**
1. Smallest change that breaks the starvation (e.g. bounded retries then briefly take the exclusive path, or
   throttle SPU polling of lines recently written by the PPU). No game-code patches.
2. Verify: stall rate drops over >= 5 sessions; MK and BO2 still boot and play 10+ min; no new fatal errors.
3. Regression guard: boot-test MK (BLUS30522) and one more title; compare FPS.

**Step 9 - Close out**
Attach the episode folders (screenshot, dump-threads, guest dump), PDB hash, build id and the experiment table to the
issue; update `docs/RPCS3_FREEZES.md` status table.

## 4. Open questions

- Why did MK stall at ~0:41 on 20078-based builds only? (B)
- Is C the same mechanism as A, or the SPU zlib job (`ZLibCellSpursKernel0`) running far slower than on hardware?
- What loads the SPURS kernel image that jumped to zeroed LS `0x28984` (D), and why only with the game patch?
- Does 4-player play change stall frequency? (Never measured.)
