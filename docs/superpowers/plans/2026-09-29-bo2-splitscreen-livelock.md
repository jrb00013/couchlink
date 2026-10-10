# BO2 Split-Screen Livelock Fix Plan

**Goal:** Play BO2 (BLUS31011) split-screen through CouchLink without the 0x17 crash or the soft freeze after grenade/SPURS spikes.

**Status (2026-09-29):** Crash suppressed by `BLUS31011_patch.yml` (hang-detect store NOPed at `0x41c414`, flag read forced to 0 at `0x41c3cc`). Soft freeze is unpatched: main thread spins in a wait loop `0x73d6ec..0x73d7b4`, waiting on a SPURS completion.

## Known facts (from logs, not guesses)
- Zombies black freeze: log shows map load finishing (~3:24), then no errors, only a stall. The pack's per-game config (`config_BLUS31011.yml`) applied heavy accuracy settings to the whole title, incl. Zombies. **Suspected, unconfirmed.**
- The pack config is currently renamed to `config_BLUS31011.yml.splitscreen-pack.disabled` in `RPCS3/config/custom_configs/`. Do NOT restore it until Task 1 proves it is safe for Zombies.
- No decrypted `t6mp` ELF exists on this machine and there is no PPC disassembler.
- Patch hash `PPU-fa8ffe9a...` applied in the Zombies session too, so the patch is not MP-only.

## Task 1: Prove or clear the config as the Zombies freeze cause
- [ ] Zombies solo, pack config disabled, global config: does it load and play? (Record yes/no + log.)
- [ ] Re-enable ONLY `Max SPURS Threads: 4` in a copy of the config, retest. Then only `Accurate SPU DMA`, then only `SPU Wake-Up Delay: 20`. One knob per run.
- [ ] Outcome: a per-mode config split (MP/Zombies use global; split-screen-only overrides), or drop the offending knob. Update `install.sh` and `config_BLUS31011.overrides.yml` to match.

## Task 2: Get code to read (unblock RE)

**Prepped:** `tools/bo2-re/` has `gdb_dump.ps1` (Windows-side GDB memory dump), `disasm.py` (capstone PPC64 BE, self-tested) and `RUNBOOK.md`. The GDB stub only listens while a game runs, so this needs one BO2 boot.
- [ ] Decrypt `t6mp_ps3f.self` with RPCS3 GUI (Utilities → Decrypt PS3 binaries), save the ELF into the scratchpad.
- [ ] Install a PPC disassembler (`pip install capstone`, arch PPC64 big-endian) and dump `0x73d6ec..0x73d7b4` plus callers.
- [ ] Alternative if decrypt is blocked: RPCS3's debugger / Kernel Explorer on the live frozen process; PPU disasm view at `0x73d6ec`.

## Task 3: Identify the wait loop and its exit condition
- [ ] From the disassembly: what value/lwsync-load does the loop test? Which SPURS event/flag is it waiting on?
- [ ] Confirm with a log line or breakpoint that the flag never changes during the freeze.

## Task 4: Write the survival patch
- [ ] Candidate A: bounded spin — patch the loop's branch so it exits after N iterations (needs a free counter register or a code cave).
- [ ] Candidate B: make the wait treat "not ready" as "ready" once the hang-detect timer would have fired.
- [ ] Add to `BLUS31011_patch.yml` as a separate, individually-toggleable patch entry; keep the crash patch independent.

## Task 5: Verify and ship
- [ ] Split-screen Zombies + MP, trigger grenade/SPURS spike, confirm no freeze for 10 minutes.
- [ ] Update `contrib/rpcs3-bo2-splitscreen/README.md`, run `install.sh` against a scratch RPCS3 dir.
- [ ] Commit, push, PR (co-authors dpoin23, riccowrld).

## Review Focus
- Patch must not break single-player/MP (verify MP solo boots).
- Bounded-spin exit must not fire during normal loads (false positives skip real waits).
- `install.sh` must back up any existing custom config before overwriting.
