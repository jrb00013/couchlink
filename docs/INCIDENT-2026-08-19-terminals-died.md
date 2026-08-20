> **Status (2026-08-19, later same day): fixes #1 and #3 implemented.**
> win-capture now launches via a Scheduled Task instead of a `Start-Process`
> child of the WSL session (`scripts/ensure-win-capture.sh`) — it is no
> longer a member of Windows Terminal's job object, so a crashed/closed
> terminal cannot take it down with it. The host also now self-heals: if the
> capture link stays down for more than 5s, it re-invokes
> `ensure-win-capture.sh` itself (`crates/host/src/capture/{bridge,hyperv_bridge,mod}.rs`,
> `respawn_windows_capture`), closing the "waited forever for a reconnect
> that nothing triggers" gap directly. Fix #2 (persistent win-capture log
> file) and fix #4 (track the Windows Terminal XAML crash upstream) are
> still open. See "Fixes to implement" below for the original plan and
> `git log --oneline -- scripts/ensure-win-capture.sh crates/host/src/capture`
> for what actually landed.

# Incident: all 8 terminals died at once + stream froze

**Date:** 2026-08-19, ~22:30 local (02:30 UTC)
**Reported by:** host (josep), mid-session with a remote player on PCSX2
**Impact:** every Windows Terminal window/tab closed simultaneously; the remote
stream froze (no new video); the player stayed on the emulator P2 binding but
could no longer see anything. The host session did **not** exit.

---

## TL;DR

`WindowsTerminal.exe` crashed once (faulting module `Windows.UI.Xaml.dll`,
exception `0xc000027b`). Windows Terminal hosts **every tab/window/panel in a
single process**, so that one crash killed all 8 terminals at the same time —
this is why it looked like "everything died at once."

WSL itself **never crashed** (the distro's PID 1 had ~57h of uptime at the
time of this investigation, and the host process survived and kept running,
re-parented to PID 1). The *actual* casualty that took the game down was
`couchlink-win-capture.exe` on the Windows side: its TCP link to the host
reset ~3 s after the terminal crash and nothing ever restarted it, so the
host kept running with no new frames and the player saw a frozen picture.

The terminal crash is the trigger; the missing
capture-restart + invisible win-capture logging is what turned a terminal app
crash into a dead game session.

---

## Evidence

### Windows Application event log (Application Error)

```
08/19 22:30:48  WindowsTerminal.exe, version: 1.24.2607.10001
                C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.24.11911.0_x64__8wekyb3d8bbwe\WindowsTerminal.exe
                Faulting module: Windows.UI.Xaml.dll, version: 10.0.26100.8972
                Exception code: 0xc000027b
```

`0xc000027b` is the classic "stale XAML visual tree / UWP app-crash" code —
a Windows Terminal (and its hosting of every tab) terminating from inside its
UI framework, not a deliberate close. All other recent Application Error
entries are unrelated ASUS `ArmouryCrate` / `LightingService` crashes.

### WSL / host side (`.run/host-cf-live4.log`)

Timeline (all times UTC; local = UTC−4):

| Time | Event |
|------|-------|
| 02:30:11 / 02:30:13 | `frame push exceeded budget — dropped, asked for a keyframe` |
| 02:30:15 | link governor: 47% sheds → commanded `1280x720@15 (5000 kbps)` |
| 02:30:20 | governor: 6% sheds → `2500 kbps` |
| 02:30:31 | governor: 0% sheds → `5000 kbps` |
| 02:30:36 | governor: 5% sheds → `2500 kbps` |
| 02:30:46 | governor: 0% sheds → `5000 kbps` |
| **02:30:51.965** | `WARN couchlink_host::capture::bridge: Windows capture client lost (frame magic: Connection reset by peer (os error 104)) — waiting for reconnect` |
| 02:30:52 | `webrtc_sctp: unable to parse SCTP packet chunk too short` ×2 |
| 02:33:39 | player left slot 1 (WebRTC peer closed) |
| 02:33:40 | player rejoined slot 1 (epoch 2), reconnected, pad + video DC open |
| 02:33:56+ | pad re-announced, emulator binding re-asserted (`XInput Pad #1`, PCSX2 Pad2 `already` bound) |
| **never** | `Windows capture client reconnected` — **win-capture never came back** |

Host process liveness: still running after the incident (re-parented to PID 1),
`/proc/uptime` ≈ 205k s (~57h), so **WSL did not restart**.

Windows process check after the incident: `Get-Process couchlink-win-capture`
returns nothing → win-capture.exe is gone and stayed gone.

---

## Root-cause chain

1. **`WindowsTerminal.exe` crashed** (XAML, `0xc000027b`) at 22:30:48 local.
   Because one terminal process hosts every tab, **all 8 terminal
   windows/tabs closed in the same instant** — the user's "why did all my
   terminals crash."

2. **win-capture died as collateral.** `couchlink-win-capture.exe` was
   launched from a terminal-resident chain (WSL → `run.sh` →
   `start-host.sh` → `ensure-win-capture.sh` → `Start-Process`). Its TCP
   connection to the WSL host reset ~3 s later (02:30:51 UTC =
   22:30:51 local). The capture picker process and its console were torn down
   with the crashed terminal tree.

3. **The host survived but had no frames.** `WindowsBridge::capture()`
   (`crates/host/src/capture/bridge.rs`) handles a dead client gracefully —
   it keeps the session alive, serves the last frame, and waits for a
   reconnect. That is correct behaviour; the problem is the reconnect never
   happened because **nothing relaunches win-capture**. `ensure-win-capture.sh`
   runs exactly once, at host startup (`scripts/start-host.sh:54`); there is
   no watchdog on either side.

4. **The player experience degraded silently.** With no frames, the browser
   sat on stale/frozen video. The player left at 02:33:39, rejoined, but
   capture was still down → still frozen. From the host's seat, "the whole
   thing crashed" — but the host process was alive the entire time, just
   streaming nothing.

---

## Why it is invisible (investigation gaps that made this hard)

1. **win-capture has no file logging.** `start-win-capture.ps1` runs it in a
   minimized console; its `tracing` output goes nowhere when that console
   dies. We cannot see the exact final error from the exe — only the Windows
   crash dump of the terminal and the host-side TCP reset.
2. **No watchdog / restart path for win-capture.** The host bridge waits for
   a reconnect that nothing will ever trigger. A single `ensure-win-capture.sh`
   invocation at boot is the only launch.
3. **No console-close handling in win-capture.** It inherits a console from
   the launching terminal and does nothing to survive (or detach from) it.
   When the terminal died, it went with it.

---

## Fixes to implement (root-cause, not restart bandaids)

The user asked for the diagnosis to drive a real fix — auto-restart alone is
a band-aid. Address the *structure*, in priority order:

### 1. Decouple win-capture from terminal lifetime (the actual kill vector)

- Launch `couchlink-win-capture.exe` **fully detached** from the terminal:
  `CreateProcess` with `DETACHED_PROCESS` / `CREATE_NEW_PROCESS_GROUP`, or a
  Windows service / `schtasks`-style background job, instead of
  `Start-Process -WindowStyle Minimized` inside a terminal-owned tree.
- Handle `CTRL_CLOSE_EVENT` / `CTRL_LOGOFF_EVENT` explicitly in the exe and
  ignore them, so a dying console can't take the capture down.
- This is the fix for "why did all my terminals crashing kill the game" — the
  capture must not live in the terminal's process tree at all.

### 2. Make win-capture self-healing / observable

- Persist `tracing` to a log file on Windows
  (`%LOCALAPPDATA%\couchlink\logs\win-capture.log`) so the real final error is
  always available.
- Have the exe exit with a **non-zero code + reason string** on capture-source
  close / encoder failure, and have the launcher treat that as "restart with
  backoff", not silence.

### 3. Host-side supervision (last line of defence)

- When `WindowsBridge` detects the client is gone, have the host re-run
  `ensure-win-capture.sh` after a short delay instead of only waiting for a
  reconnect that may never come. Pair it with the old-frame timeout already
  present so a dead capture can't masquerade as a healthy session.

### 4. Watch the terminal itself (the upstream trigger)

- `WindowsTerminal.exe 1.24.11911` faulting in `Windows.UI.Xaml.dll` is an
  app bug. Track it, and if it recurs, pin an earlier Windows Terminal
  version or switch the affected sessions to a different host. This alone
  does not fix the session — the real fix is #1–#3 — but a terminal that
  crashes once should not be able to end a game.

---

## Verification plan (for whoever implements)

1. After a fix, kill `couchlink-win-capture.exe` manually mid-session → host
   must relaunch it and the stream must recover **without** the player
   rejoining (this is the current gap).
2. Close a terminal that has WSL + the stack in it mid-session → capture must
   survive; game continues.
3. Check `win-capture.log` after any of the above to confirm the exit reason
   is visible.
4. Regression: normal picker start, normal shutdown, and the existing
   reconnect-path tests in `crates/host/src/capture/bridge.rs` still pass.