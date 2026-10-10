# BO2 split-screen livelock: RE runbook

Facts (issue rpcs3#16426 dump, BLUS31011 01.00, PPU hash `PPU-fa8ffe9a...`):
- `main_thread` spins at `0x73d6ec..0x73d7b4`: `ldarx/stdcx` CAS, `lwarx/stwcx` exchange,
  `bl 0x73d09c(*(0xccc740), 0, 1, r30)`, `sys_timer_usleep(50)`, repeat. Reservation `0x2e208a0`.
- Call stack: `0x73d7a8 <- 0x3aca08 <- 0x6dd2d4 <- 0x125128 <- 0x125538 <- 0x30084c <- 0x300b18 <- 0x41c18c`.
- It is a wait for a job/completion flag that never gets set. Not known: which thread should set it.

## One-time check (no game needed)
    python3 tools/bo2-re/disasm.py --selftest

## Capture (needs BO2 running)
**Connecting to the GDB stub PAUSES the emulator.** Only run this while the game is already frozen/expendable. `gdb_dump.ps1` sends continue on exit, but that resume path is untested; if the game stays paused use RPCS3 Emulation > Resume. One connection per boot.
1. Boot BO2 with the crash patch (`BLUS31011_patch.yml`) so the watchdog cannot kill the process.
2. Reproduce the freeze (split-screen, grenade / SPURS spike).
3. While frozen, from WSL:
       powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w tools/bo2-re/gdb_dump.ps1)" -Out C:\Users\josep\bo2dump
   (Config `GDB Server: 127.0.0.1:2345` is already set; RPCS3 only listens while a game runs.)
4. In RPCS3: Debug > Kernel Explorer. Screenshot/copy: every PPU thread's state + wait object,
   the SPURS/SPU thread group state, semaphores/event queues with waiters. THIS names the
   missing producer; the memory dump alone cannot.
5. Disassemble:
       python3 tools/bo2-re/disasm.py /mnt/c/Users/josep/bo2dump/wait_loop.73d000.hex --from 73d690 --to 73d7c0

## Decide (after step 4-5)
- Producer thread exists but is blocked on X  -> real deadlock; patch the producer's wait or the flag write.
- Producer SPU group stopped                  -> SPU-side starvation; try config (Max SPURS Threads, Wake-Up Delay) first.
- Everything waits on the main thread's flag  -> lost wakeup; patch below.

## Patch candidates (write only after disassembly; addresses below are placeholders until then)
A. Bounded spin: replace the loop's back-branch with a conditional that exits after N passes.
   Needs a spare register/counter or a code cave in `.text` padding.
B. Force-ready: make the `0x73d09c` result read as "done" once the watchdog heartbeat is stale
   (heartbeat word at `0xcbcd08`).
Ship as a separate entry in `contrib/rpcs3-bo2-splitscreen/BLUS31011_patch.yml`, toggleable
independently of the crash patch.
