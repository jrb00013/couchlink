# RPCS3 "frozen" runbook (Mortal Kombat, BO2) and CouchLink stream-black runbook

Written after the 2026-09-30/10-01 incident. Most "freezes" that night were **not** deadlocks, and
every restart destroyed the evidence. Read the table, run the script, **do not kill RPCS3 first**.

## 1. First response (30 seconds)

```bash
tools/rpcs3-debug/rpcs3-hang-diag.sh            # verdict + evidence, non-invasive
tools/rpcs3-debug/rpcs3-hang-diag.sh --guest --dump --shot   # while it is STUCK: all evidence
```

| Verdict | Meaning | Do |
|---|---|---|
| `LOADING` | No FPS in title, many cores busy. PPU/LLVM, shader-interpreter precompile or SPU compile. Looks exactly like a freeze (log goes quiet, the overlay shows "Applying PPU Code" / "Building base variant N of M"). | **Wait.** Killing restarts the compile. Check the screen (`--shot`). |
| `GUEST_STALL` | RPCS3 alive and rendering, but the game logged nothing for 30 s+. A guest wait loop (spinning `sys_timer_usleep`). Observed: MK story-mode cutscene, stuck ~2 min, then recovered by itself. | Wait 3-5 min. If still stuck, capture evidence with `--guest --dump --shot` **before** restarting. |
| `DEADLOCK` | Everything idle, no FPS. | Capture evidence first (`--dump --shot`), then restart. |
| `NOT_RUNNING` | Process gone. Script prints the last fatal lines. | Read the log; BO2 crash signature is `Access violation ... 0x16/0x17`. |
| `HEALTHY` | Emulator fine. If friends see a frozen/black picture the problem is the stream: section 4. | |

Facts that cost hours:

- **FPS in the window title is not proof of life.** During the MK cutscene stall the title kept saying
  59.91 FPS (RSX kept presenting the same frame) while the game was frozen.
- **`sys_timer_usleep` rate is not a stall signal.** Healthy MK does 17-50k/s; the stall was ~62k/s.
  The script prints it for information only.
- **Game-event silence is the signal that separated them**: healthy play = a game log line every few
  seconds, stalled cutscene = 66 s of nothing except audio-port churn.
- A stall's **cutscene end** looks like: `cellVdecEndSeq` then, normally, `cellVdecClose` ~1 s later. In the
  stall `cellVdecClose` came ~2 min later.
- The `Exclusive Fullscreen Mode: Prefer borderless fullscreen` CFG error at boot is cosmetic.
- WSL launching Windows `.exe` can time out (`UtilAcceptVsock accept4 failed 110`); the script retries and
  falls back to log freshness. `wsl --shutdown` fixes it but kills the CouchLink stack.

## 2. Thread-level debugging (always available)

Three layers, cheapest first. All work on a **live, stuck** process.

1. **Native threads, named** (`threads.ps1`, used by the diag script): per-thread CPU over N seconds with
   RPCS3's thread names (`rsx::thread`, `PPU[0x1000000] main_thread`, `RenderingThread`,
   `SPU[...] MwyCellSpursKernel0`). Tells you who is burning cores. Non-invasive.
2. **Guest PPU thread dump** (`--guest`): RPCS3 fork builds from `couchlink-play-19984` @ `7fd007b`
   or later watch for `<RPCS3 dir>\dump_threads.trigger`. Creating it logs every PPU thread's context
   (PC/LR/callstack, syscall + call history) to `RPCS3.log` within 1 s, with no pause. This is how you find
   the guest address a stuck thread is looping at (then disassemble with `tools/bo2-re/disasm.py`).
   *Not yet run against a real stall* (the build was still compiling when this was written).
3. **Native minidump** (`--dump`): full `comsvcs.dll MiniDump`, pauses the process a few seconds. Keep the
   matching `rpcs3.pdb` (CI uploads artifact `RPCS3 Windows MSVC PDB`, added in `bb0b7b7`) and resolve
   RVAs with `tools/rpcs3-debug/symbolize.py rpcs3.pdb 0x7ab562 ...`. Needs WinDbg/VS to open the dump.

Avoid the **GDB stub** (`GDB Server: 127.0.0.1:2345`, `tools/bo2-re/gdb_dump.ps1`) for first response: it
pauses the whole emulator on connect and is single-shot per boot.

## 3. Known causes and status

| Item | Status |
|---|---|
| MK stalls ~0:41 into boot on **every 20078-based build** (hidfix, cbwait, plain 20078); fine on 19984-based builds | **Root cause unknown**, somewhere in upstream 19984..20078. Not config, HID lock, CB-wait, SPURS fix, or worker count (19984 also picks 12). Workaround: use the `19984-play` build (`switch-rpcs3.cmd couchlink-play`). |
| Shader-interpreter precompile waiting forever on a stalled worker | Mitigated: 30 s no-progress watchdog in `VKShaderInterpreter.cpp` (`couchlink-play-19984`). On a cut-short precompile, remaining variants compile on demand. |
| BO2 present freeze `CB chain has run out of free entries` | Fixed in `couchlink-play` (Vulkan CB wait/reclaim, ring 1024). |
| BO2 softlock (wait loop at `0x73d6ec`) | Game patch in `contrib/rpcs3-bo2-splitscreen` (force-complete then exit). **Never run in-game yet.** The one-poll-exit variant crashed (`Access violation 0x16`). MT RSX must stay off for BO2. |
| MK cutscene stall (~2 min) | Self-recovered. Cause unknown. Next occurrence: `rpcs3-hang-diag.sh --guest --dump --shot` while stuck. |
| RSX `semaphore_acquire timed out` at ~0:40 | Consequence of a long RSX-thread stall (e.g. precompile), **not** a cause. Do not change TDR behavior for it. |

Do not "fix" by editing config: per-game config changes did not change any of the above.

## 4. CouchLink stream shows black / "WebCodecs video" with no picture

Cause that night: host connected, **capture on the wrong window**. Checklist, in order:

1. `.env.couchlink` pins `COUCHLINK_CAPTURE_WINDOW` (`31011` = BO2, `30522` = MK). It **overrides** the
   command-line env. Wrong needle = viewers connect, no frames.
2. A hidden PowerShell supervisor (`start-win-capture.ps1 -Window <old>`) respawns
   `couchlink-win-capture.exe` with its original args, and `ensure-win-capture.sh` then says "already
   running - leaving it alone". Kill the supervisor tree (`taskkill /PID <ps> /F /T`), not just the exe.
3. If the Hyper-V link flaps (`link lost / treating as dead`) use the TCP transport:
   `COUCHLINK_CAPTURE_TRANSPORT=tcp`; verify `ss -tan | grep :9876` shows `ESTAB`. In TCP mode the host
   prints no streaming-FPS line.
4. Restarting the stack changes the trycloudflare hostname: **old links are dead**.
5. Never `pkill -f 'couchlink-host|run.sh host'` inline in a shell: the pattern matches the shell itself
   and kills it (exit 144).

All of that is encoded in:

```bash
scripts/restart-stack-for-game.sh mk      # or bo2, or a window-title needle
```

(`.env.couchlink` is untracked; the script edits it. Previous value of the BO2 needle: `31011`.)

## 5. Prevention

- Always check `LOADING` before killing; relaunch loops kept the PPU/shader work from ever finishing.
- Keep one RPCS3 session per diagnosis: evidence (stack, log, screen) dies with the process.
- Build artifacts must include the PDB; keep the PDB next to the exe you staged.
- Verify against the **screen and the log**, not the title FPS. State "unverified" in PRs until a game has
  actually been run on the exact binary.
