# BO2 test night protocol

Before friends join: `tools/rpcs3-debug/preflight-bo2.sh` - fix every FAIL (WARNs are informational). It prints the join link.

While playing (host or anyone): when it freezes
1. **Do not restart.** Note the wall time, how many players, what was happening (lobby / party / match / loading).
2. Wait. Every stall so far ended by itself (20 s - 7 min). The Windows-native watcher captures everything automatically.
3. If it passes 10 minutes, run `tools/rpcs3-debug/rpcs3-hang-diag.sh --shot --guest` and tell me the time.

If friends see no video: `scripts/capture-keeper.sh` restarts a dead capture automatically within ~1 min;
manual: `scripts/restart-stack-for-game.sh bo2` (new link!).

After the session: `tools/rpcs3-debug/summarize-stalls.sh 1800` (episodes since 18:00, durations, hottest stwcx. lines).
Send me that output plus: player count, whether anyone was in a party/lobby, any crash dialog text.

What counts as progress tonight (be honest in the notes): number of episodes per hour and median length with N players;
whether stalls cluster in the lobby/party; whether the stwcx. histogram (build >= 40500ef) shows one hot line.

## Tonight's A/B (build `19984-play4`, commit c96e609 on the fork)

The build adds one emulator change: SPU `GETLLAR`/`PUTLLC` back off ~10 us on a 128-byte line where a PPU
`stwcx.`/`stdcx.` failed in the last ~50 us, **only when `PPU Reservation Priority Over SPUs` is on** (BO2's config has it on).

Baseline (build `19984-play2`, 2026-10-01 11:00-12:00, one player + lobby): **11 stall episodes/hour, median ~35 s, longest ~435 s.**

| Run | Setting (`config/custom_configs/config_BLUS31011.yml`) | What to do |
|---|---|---|
| A (fix on) | `PPU Reservation Priority Over SPUs: true` (as installed) | Play >= 30 min, same kind of activity (lobby/party/match) |
| B (fix off, only if A is unclear) | set it to `false`, relaunch BO2 | Same duration and activity |

Record for each run: minutes played, players, `summarize-stalls.sh` output. Compare episodes/hour, median and max length.

**Pass:** clearly fewer or much shorter stalls in A than the baseline/B (e.g. median under ~10 s, nothing over ~60 s).
**Fail:** no change, or A is worse (SPU slowdown/low FPS): turn the option off and tell me. Do not conclude from one session.
**Engagement check:** after any session, `rpcs3-hang-diag.sh --guest` prints the line
`stwcx./stdcx. failures ... (SPU back-offs for PPU priority, total: N)`. N = 0 means the change never ran (wrong build or option off).
Also grep `RPCS3.log` for `PPU reservation contention`: it fires at >= 20k stwcx. failures per 10 s with the 4 hottest lines.

### First numbers from build `19984-play4` (fix on), 2026-10-01 18:15, 2:07 into a fresh BO2 boot (no stall)

- stwcx./stdcx. failures since boot: **14.3 million** (the previous build, fix not present, had ~1.0 million at 2:59). Hottest lines: `0x0269e080` 7.25M, `0x0267a000` 6.98M.
- SPU back-offs: **44 million** (~350k/s). Each SPU spends roughly half its time in the 10 us back-off.
- FPS held at 60; no stall during this window.

Read: the change engages heavily and the PPU failure count went **up**, not down, so on those two lines the PPU is probably losing to something
other than SPU polling (or the PPU simply retries faster now). Do not trust this change until the A/B says so. Switch with
`tools/rpcs3-debug/set-prio.sh off` (then relaunch BO2) for run B.
