# Agent handoff: CouchLink + RPCS3 co-play (BO2 / MK / Sonic)

Written 2026-10-04 ~00:35 local. Read this first, then `docs/RPCS3_FREEZES.md`, `docs/RPCS3_STALL_DIAGNOSIS_PLAN.md`,
`tools/rpcs3-debug/TEST_NIGHT.md`, and the memory files in `~/.claude/projects/-home-josep-projects-couchlink/memory/`.

## What the user wants
Stable co-play over CouchLink with friends: BO2 (BLUS31011), Mortal Kombat (BLUS30522), Sonic & SEGA All-Stars Racing (BLUS30342).
- Fix BO2 freezes at the **root cause** in the RPCS3 core. No config experiments, no log-only changes, no bandaids, no game patches.
- MK and CouchLink must never regress in latency or input response ("fast as f").
- Fixes must be per-game / dynamic, not hardcoded.
- On each request ("start up X"): close the current game, launch the requested game + CouchLink stack, **copy the join link to the clipboard and print it**.
- Eventually BO2 with 4 players (untested).
- The user is blunt and plays live with friends. Don't claim a fix works without evidence; don't invent freezes.

## Standing rules (user corrections)
- **Never cancel a CI build** (`gh run cancel`) or close a game the user is playing without asking. Let in-flight CI finish.
- RPCS3 PRs/issues go to the fork `jrb00013/rpcs3` only, never upstream.
- Do not run the live monitor on MK. Do not experiment with configs. Do not revert code BO2 needs.
- Never use the GDB stub on a live session; never take a full minidump (37 GB, suspends the game).
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
- Avoid `pkill -f` / `pgrep -f` patterns that match your own shell (it kills you, exit 144). Use exact names or PIDs.
- Windows PowerShell reads BOM-less .ps1 as ANSI: no `·` literals in .ps1 files.

## Current state (verify, do not trust)
- Active RPCS3 build: `C:\Users\josep\RPCS3\versions\19984-play12` (switch-rpcs3.cmd alias `couchlink-play` -> `19984-play12`).
  Fork branch `couchlink-play-19984`, commit 9eaa7f1.
- **Pending build:** GitHub Actions run `37176795186` on `jrb00013/rpcs3`, commit `174e4fa`
  ("starvation adoption threshold 256 -> 8"), started 04:21Z, ETA ~05:10Z. When it succeeds:
  `tools/rpcs3-debug/stage-build.sh 37176795186 19984-play13 couchlink-play-19984`, then (RPCS3 closed, and only with the user's OK)
  `cmd.exe /c "switch-rpcs3.cmd 19984-play13"` from `/mnt/c/Users/josep/RPCS3`. Update the alias line in `switch-rpcs3.cmd`
  (`set "V=19984-play12"`) if the user wants it as default. Check `gh run list -R jrb00013/rpcs3 -b couchlink-play-19984`.
- At the time of writing Sonic was running with a live stack; the user had just asked to switch to **MK** and give the link
  (that action was interrupted, not done). Current join link may be stale: always regenerate with the restart script.

## How to start a game + stack
```
# close RPCS3 first (ask if the user is mid-game), then launch the game:
powershell.exe -NoProfile -Command 'Start-Process -FilePath "C:\Users\josep\RPCS3\rpcs3.exe" -WorkingDirectory "C:\Users\josep\RPCS3" -ArgumentList "`"C:\Users\josep\RPCS3\games\<folder>`""'
# then the stack (needle: mk | bo2 | 30342 for Sonic | any window-title substring):
bash scripts/restart-stack-for-game.sh mk
```
Game folders: `Mortal Kombat (USA) (En,Fr,De,Es,It,Pt)`, `Call of Duty - Black Ops II (USA) (En,Fr)`,
`Sonic & Sega All-Stars Racing (USA, Asia) (En,Fr,Es)`. Capture needles: 31011 BO2, 30522 MK, 30342 Sonic.
The script builds host+signaling from the current checkout, kills capture + supervisor, pins the needle in `.env.couchlink`,
uses the tcp transport, and prints the join link (also in `/tmp/couchlink-join-url.txt`). Run it detached
(`setsid nohup ... > $CLAUDE_JOB_DIR/tmp/x.log &`); it can take a minute.

**Verify before handing over the link:**
1. Window title FPS ~60: `powershell.exe -NoProfile -Command '(Get-Process rpcs3).MainWindowTitle'`
2. Capture bound to the right window: Win32_Process command line of `couchlink-win-capture.exe` shows `--window <needle>`
3. Bytes flowing: `ss -ti state established '( sport = :9876 )' | grep -o 'bytes_received:[0-9]*'` twice, 4 s apart
4. Tunnel: `curl` the hostname from `/tmp/couchlink-join-url.txt` -> 200 (hostname changes whenever the tunnel restarts)
5. Native stall watcher running (`rpcs3-stall-watch.ps1`)
6. `tr -d '\r\n' < /tmp/couchlink-join-url.txt | clip.exe`, then print the full link in the reply.
"capture not attached" printed early by the script is usually just startup; re-check after ~15 s.
Gotchas: `memory/couchlink-stack-restart-gotchas.md`. To tear everything down: `bash /tmp/shutdown-all.sh` (recreate from git history if missing).

## The BO2 freeze: what is established
Fork core file: `rpcs3/Emu/Cell/PPUThread.cpp` (`ppu_store_reservation`, `note_stcx_fail/ok`, `ppu_stcx_wait_for_ppu`),
`PPUThread.h` (`stcx_streak`), `SPUThread.cpp` (`spu_yield_to_ppu_atomic` at GETLLAR/PUTLLC),
`lv2/lv2.cpp` (guest dump + contention log), `system_config.h` (`PPU Starvation CAS Refresh`, per-game, default false;
enabled for BO2 in `contrib/rpcs3-bo2-splitscreen/config_BLUS31011.overrides.yml`).

**Traced root cause of the 2026-10-04 00:19 freeze (real, user was playing):**
- Only one PPU thread running: `Secondary` looping `lwarx/addic/stwcx.` at `0x73d30c` on a SPURS counter at `0x2e208dc` (line `0x2e20880`).
  `main_thread` just polls `sys_timer_usleep(0x32)` at `0x73d7a8` waiting for it. SPU kernels are the only other writers.
- ~49M stwcx failures per 10 s (~4.9M/s), 95% `locked8` (line lock held by the six SPURS kernels), line `0x2e20880` = 43.8M of 48.2M.
- The CAS refresh and the bounded lock-wait both required `stcx_streak >= 256` consecutive failures. Successes interleave
  (~1 per 50 attempts) and each resets the streak, so the gate opened for ~0.5% of failing stores
  (~55 refreshes, ~5k lock-waits per 10 s vs 49M failures). The fix existed but almost never ran.
- Fix (commit `174e4fa`): threshold -> named constant `stcx_adopt_streak = 8` (same as the SPU handoff threshold). Per-game gated.
- **UNVERIFIED** that this ends the freeze. The ~50 attempts/success figure is derived from counters, not measured directly.

**Do not repeat my mistake:** `tools/rpcs3-debug/live-monitor.py` flags "STALL" after 15 s of no non-noise game log lines. An idle
menu with nobody playing also logs nothing, so most "stalls" I reported while the user was AFK were false alarms.
Only count a freeze when the user says it froze during play, or the contention counters are in the tens of millions per 10 s.
FPS dips to ~4 on map loads are normal. Real-freeze signature: user says frozen + one PPU thread running + huge failure rate.

Capture a freeze: `: > /mnt/c/Users/josep/RPCS3/dump_threads.trigger`, wait ~7 s, read the last ~9000 lines of
`/mnt/c/Users/josep/RPCS3/log/RPCS3.log` (PPU/SPU thread dump, kernel event queues, contention histogram). Save to
`~/rpcs3-stalls/<timestamp>/`. Previous episodes: `~/rpcs3-stalls/20261004-0008/` (idle false alarms), `20261004-real1/` (real).

## Next steps
1. Wait for run `37176795186`, stage as `19984-play13`, verify MK still boots (config for MK has no overrides; option default false),
   then BO2 with the user playing 10+ minutes. Watch `refreshes`/`lock-waits` and failures per 10 s in the contention log lines.
2. If freezes persist: look at why 'locked8' dominates. The line lock is held by SPU PUTLLC commits; check whether the SPU side
   (`SPUThread.cpp` PUTLLC path) can be made to not re-lock immediately while `want_tsc` is set, rather than only pausing before GETLLAR/PUTLLC.
3. If it works: close issue #3 on the fork with before/after numbers, update `docs/RPCS3_FREEZES.md`, open/merge the fork PR (#2).
4. Then test 4-player BO2.
5. Uncommitted work in this repo: none known. Branch `docs/rpcs3-freeze-runbook`; the RPCS3 checkout is `~/projects/rpcs3`.

## CouchLink side (done and deployed, don't redo)
Signaling auto-reconnect and video-stall watchdog (`web/src/player.ts`), encoder bitrate applied in place with last-good fallback
(`crates/capture-bridge`), tunnel watchdog needs N consecutive failed probes, capture log + heartbeat in
`%LOCALAPPDATA%\couchlink\logs\win-capture-*.log`, restart script builds from the current checkout. 139 web tests pass.
