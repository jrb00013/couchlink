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
