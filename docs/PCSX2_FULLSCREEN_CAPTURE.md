# PCSX2 F11 (fullscreen) capture freeze

Status: OPEN. Partially mitigated by PR #107 (commit a8c4443), not solved.
Labels used below: CONFIRMED = directly shown by a log or the code;
HYPOTHESIS = plausible, not proven; UNKNOWN = no evidence either way.

## Summary

In PCSX2 (Marvel Ultimate Alliance), pressing F11 makes the Windows Graphics
Capture (WGC) yellow border disappear and the remote viewer's video freezes
until F11 is pressed again. Logs show F11 does not resize the game window. PCSX2
hides it and creates a second, same-titled borderless window with a new HWND.
Every toggle makes a new HWND. The a8c4443 watcher detects the swap and
re-attaches, but the capture item for the fullscreen window closes almost
immediately (about 20 ms to 200 ms after attach in the logs), so the stream
does not recover on the fullscreen window. The root cause of that immediate
close is not proven.

## Symptom

User report: F11 in PCSX2 removes the yellow WGC border and the remote video
freezes until F11 is pressed again to leave fullscreen.

Caveat on "the host looks fine": after each capture exit the host log shows
`streaming ~57.8 fps` again within a second or two. The capture process
replays the last real frame when no new one arrives (see `replay_target` in
`win_capture.rs`), so host fps does NOT prove live video. The honest signal is
the `N new from window` counter in the capture log's `encoded ... fps` line.

## Evidence

### f11watch.log (`C:\Users\josep\f11watch.log`)

Three visible-window observations, one F11 in and one F11 out:

```
23:44:18.632 ... 3609304 [Qt6111QWindowIcon] 'Marvel - Ultimate Alliance' -9,-9,2569,1552 style=97cf0000 ex=100 iconic=False
23:44:25.796 ... 1773998 [Qt6111QWindowIcon] 'Marvel - Ultimate Alliance' 0,0,2560,1600 style=96080000 ex=0 iconic=False
23:44:37.730 ... 3609304 [Qt6111QWindowIcon] 'Marvel - Ultimate Alliance' -9,-9,2569,1552 style=97cf0000 ex=100 iconic=False
```

- CONFIRMED: windowed game window is HWND 3609304, style `97cf0000`
  (WS_OVERLAPPEDWINDOW-class, has caption/frame).
- CONFIRMED: F11 produces a different HWND 1773998 at 0,0,2560,1600 with style
  `96080000` (WS_POPUP-class, no caption/frame). Leaving fullscreen shows the
  original 3609304 again.
- The log has no columns for visibility of the non-listed window, so "3609304
  is hidden during fullscreen" is inferred from it not appearing at 23:44:25
  and from the capture logs ("hwnd 3609304 hidden").
- UNKNOWN: whether the watcher polled finely enough to see a short-lived
  intermediate window (see hypothesis H1).

### Capture logs (`C:\Users\josep\cap*.out`, ANSI stripped)

cap0, end of file, after about 1 minute of healthy capture (`capture 57.8 fps
arrived`, `282 new from window`):

```
05:39:27.755 WARN hwnd 3609304 hidden; replacement hwnd 12390362
05:39:27.756 WARN captured window hidden and another matching window is visible (fullscreen toggle?) — exiting to re-attach
```

cap1 attaches to 12390362, encodes (`output frame 0/1`), then 1.4 s after
start:

```
05:39:29.010 WARN hwnd 12390362 hidden; replacement hwnd 3609304
```

cap2 re-attaches to 3609304 and runs for 58 s, then:

```
05:40:27.296 WARN hwnd 3609304 hidden; replacement hwnd 6689514
```

cap3 attaches to the fullscreen window and dies at once:

```
05:40:27.795 INFO capture target hwnd=6689514 visible=true
05:40:28.014 INFO GPU scale+convert active: 2560x1600 -> 1152x720 NV12, no readback
05:40:28.031 WARN capture source closed — exiting so the launcher re-resolves the window
```

That is 17 ms after the first GPU frame and 236 ms after attach. This is the
`on_closed` path (WGC `Closed` event), not the hidden-window watcher.

cap5 (about 1 hour later), a second failure mode:

```
06:30:27.495 INFO capture target hwnd=2823258 visible=true
06:30:27.698 INFO keep_rendering: captured window closed — keep-rendering watcher stopping
06:30:27.698 WARN capture ended: Failed to convert item to `GraphicsCaptureItem` — waiting for window to reappear
06:30:27.699 INFO capture target hwnd=3609304 visible=true      <- falls back to the OLD window
06:30:28.248 WARN hwnd 2823258 hidden; replacement hwnd 3609304
```

Here `IsWindow(2823258)` went false within about 200 ms (keep-rendering saw
"closed"), and WGC could not create an item for it.

HWNDs seen across toggles: 3609304 (original), 12390362, 6689514, 2823258,
1773998 (the last from f11watch only). CONFIRMED: every toggle creates a new
HWND; the windowed HWND 3609304 persists and is re-shown on exit.

cap4 re-attaches to 3609304 (`visible=true`, `cropping to client area
2542x1499`) and runs about 50 minutes at roughly 270 `new from window` per
5 s, so windowed capture is healthy.

### Host log (`/tmp/couchlink-stack-20261106-232749.log`)

```
05:39:27.556 WARN Windows capture client lost (frame magic: Connection reset by peer (os error 104)) — waiting for reconnect
05:39:28.840 WARN Windows capture client lost (...)
[couchlink-host] streaming 39.7 fps ...   <- reconnect gap
[couchlink-host] streaming 57.9 fps ...
05:40:27.622 / 05:40:28.384  (same message, matches cap3)
06:30:26.870 / 06:30:28.187  (same message, matches cap5)
```

CONFIRMED: each lost-client line lines up with a capture exit within about
100 ms, and host fps returns to about 57.8 within a few seconds. As noted
above, that recovery number alone does not show live video.

Not related, do not conflate: the 2026-11-08 16:41+ burst of `no frame from
win-capture in 3.0s ... relaunching it` in the same log is a different
condition (link hung/down loop) with no F11 involvement shown.

### Code (branch `fix/capture-stale-not-on-hitch`)

- `crates/capture-bridge/src/bin/win_capture.rs`
  - `find_window`: prefers a visible window among those matching the needle.
  - `spawn_hidden_watch` (added in a8c4443): polls `IsWindowVisible` every
    250 ms; if the captured HWND is hidden for 500 ms and a different visible
    match exists, `process::exit(EXIT_SOURCE_CLOSED)` (75).
  - `on_closed`: logs `capture source closed` and exits 75.
  - Replay path: `replay_target` / "Nothing new this beat? Re-encode the frame
    we already have" keeps cadence when WGC delivers nothing. This is why the
    host can show about 57 fps while the picture is frozen.
- `scripts/start-win-capture.ps1`: on exit 75, sleeps 250 ms and relaunches
  `win-capture.exe`, which re-runs `wait_for_window`/`find_window`.
- `crates/capture-bridge/src/keep_rendering.rs`: polls `IsWindow` and
  `IsIconic` on the captured HWND; logs `captured window closed` when it is
  gone. Not the cause (it is parked-minimize logic) but its log line is useful
  evidence of window destruction.

## Timeline of one F11 toggle (as observed)

| Step | HWND | Visible | Rect / style | What capture does | Source |
|---|---|---|---|---|---|
| Windowed, healthy | 3609304 | yes | -9,-9,2569,1552 / 97cf0000 | WGC item live, about 270 new frames per 5 s, crops client area | f11watch 23:44:18; cap4 |
| F11 pressed | 3609304 | hidden | n/a | WGC item stays open, frames just stop, border vanishes | cap0 `hidden`; code comment on `spawn_hidden_watch` |
| Fullscreen window appears | 1773998 / 12390362 / 6689514 (new each time) | yes | 0,0,2560,1600 / 96080000 (1773998 only) | Watcher notices after 500 ms (plus up to 250 ms poll), exits 75 | f11watch 23:44:25; cap0 05:39:27.755 |
| Launcher relaunch, attach | new HWND | yes | 2560x1600 | `capture target hwnd=...`, GPU scale+convert starts | cap3 05:40:27.795-28.014 |
| Fullscreen item closes | same new HWND | n/a | n/a | `capture source closed`, exit 75, loop repeats | cap3 05:40:28.031; cap5 06:30:27.698 |
| Fallback | 3609304 (old, hidden) | no (cap5 logs `visible=true`, see note) | n/a | Attaches to old window, `hidden` watcher fires again | cap5, cap1 |
| F11 pressed again | 3609304 | yes | -9,-9,2569,1552 | Windowed capture resumes normally | f11watch 23:44:37; cap4 |

Note: cap5 logs `visible=true` for 3609304 at 06:30:27.700 while the next
watcher line says the replacement is 3609304 at 06:30:28.249 (hidden watcher
for 2823258). The ordering suggests the player toggled back within that second.
UNKNOWN: exact player action timing, nobody was logging key presses.

## What was tried and ruled out

1. Guess: "Exclusive Fullscreen: Disallowed" setting in PCSX2 is the problem.
   Status: WRONG/UNPROVEN. No log shows the setting changing behavior, and the
   f11watch style bits (`96080000`, WS_POPUP, no `ex` flags, 0,0,2560,1600) are
   a borderless window, not exclusive fullscreen. Do not cite as a cause or fix.
2. a8c4443 hidden-window watcher (PR #107). Status: PARTIAL. CONFIRMED to work
   as designed: every `hidden; replacement hwnd` line is followed by a
   re-attach. Without it, the WGC item on the hidden window never closes
   (per the code comment: "frames just stop and the yellow border vanishes"),
   so it fixed "freeze with no recovery on swap". It does NOT fix the
   fullscreen window itself, because that window's item closes right after
   attach (cap3), so the player still has no video while fullscreen.
3. Host-fps as a health signal. Ruled out as evidence, see Symptom caveat.

## Root-cause status

- CONFIRMED: F11 hides the windowed HWND and creates a new borderless HWND.
- CONFIRMED: the WGC item for a freshly-created fullscreen HWND closed within
  20 ms to 250 ms of attach (cap3), or the HWND was already gone / not
  convertible (cap5, `Failed to convert item to GraphicsCaptureItem`).
- CONFIRMED: the failure repeats across at least 3 separate HWNDs, so it is
  not a one-off.
- HYPOTHESIS H1: PCSX2 (Qt) builds the fullscreen window in two steps, e.g.
  creates a transient window, then destroys and recreates another (or
  re-parents the render surface). Our attach lands on the transient one and
  it dies. Fits cap5 (HWND destroyed within 200 ms) and the HWND churn
  (12390362 then 6689514 then 2823258 for what should be a single toggle each).
  Unproven: f11watch saw only one fullscreen HWND (1773998) and its
  polling rate is unknown.
- HYPOTHESIS H2: WGC rejects or closes items for this borderless window (for
  example because the swap chain changes to flip-model/independent flip and
  the surface is taken over), so the item is closed by the system. Fits
  cap3 (clean first frame at 2560x1600, then close). Unproven; no HRESULT
  or `Closed` reason is logged.
- UNKNOWN: whether HWND 6689514 was still alive after the close. The log does
  not check `IsWindow` or `IsWindowVisible` at close time.
- UNKNOWN: whether the same happens in RPCS3 or other Qt emulators (code
  comment says "and other Qt emulators"; no logs here).

## Proposed fix options

### A. Monitor capture while the game is borderless-fullscreen-sized, then switch back (RECOMMENDED)

When the matching visible window's rect covers a whole monitor (here
0,0,2560,1600 on a 2560x1600 display) or attach to it fails/closes inside a
short window, switch the source to WGC monitor capture of that monitor (the
`desktop` source already exists in `start-win-capture.ps1`), optionally
cropped to the window rect. When the windowed HWND is visible again, switch
back to window capture.

- Pros: does not depend on the transient HWND surviving or on `GraphicsCaptureItem`
  creation for it. Works for H1 and H2 (monitor items survive window churn).
  Same WGC pipeline, so the existing GPU convert/encode path is reused.
- Cons: captures everything on that monitor (overlays, notifications, other
  windows above the game; privacy leak on a shared screen). Needs a source
  switch without tearing the encoder down (the earlier fps-target change in
  ddd0725 shows this is doable). Monitor capture may show the yellow border
  differently; DRM/protected content not relevant here.
- Note: if H2 is true because of independent-flip, monitor capture still works
  since DWM composes it for capture.

### B. Retry attach with back-off on the transient window

If the item closes within, say, 1 s of attach, wait 100-500 ms, re-run
`find_window`, and try again (up to N tries) before falling back to the old
hidden window.

- Pros: smallest code change. Fixes H1 outright if the final window is stable.
- Cons: useless under H2. Currently the loop is 250 ms relaunch via PowerShell,
  so it already retries in effect; cap5 shows it just falls back to the old
  window. Needs fix for "fall back to old hidden window", which wrongly reports
  success while frozen.

### C. DXGI desktop duplication

Capture the monitor via `IDXGIOutputDuplication` instead of WGC.

- Pros: independent of WGC item semantics; widely used.
- Cons: new capture backend, no per-window crop, loses WGC border control, fails
  in some secure desktop/UAC states, and only one duplication per output per
  process. More code than A for a similar result. Only worth it if monitor WGC
  (A) also fails.

### D. Workaround: tell players to use borderless windowed instead of F11

No code change, see below.

- Pros: immediate, zero risk.
- Cons: a manual step, easy to forget, does not help a friend session already
  in fullscreen.

Recommendation: A, with B's short back-off as the trigger (only switch to
monitor capture after one failed or immediately-closed window attach), and D
documented in the meantime. Confidence: moderate. A works whichever of H1/H2 is
true, but this is judgment, not a tested result. B alone is not recommended.

## Test plan

Reproduce (needs a free PCSX2 session, do not run on a live friend session):

1. Start the stack with a PCSX2 game running windowed, `-Source window
   -Window "Ultimate Alliance"`.
2. In `cap*.out`, confirm baseline: `capture target hwnd=<W> visible=true` and
   `encoded ~58 fps (~250+ new from window ...)`.
3. Press F11 once, wait 10 s, press F11 again.
4. Optionally run the f11watch script (poll every 50 ms or faster) to settle
   H1: does more than one new HWND appear during one toggle? Also log
   `IsWindow`/`IsWindowVisible`/the WGC `Closed` event at close time to settle
   H2.

Proves the bug (current behavior): `hidden; replacement hwnd`, then either
`capture source closed` shortly after `GPU scale+convert active`, or
`Failed to convert item to GraphicsCaptureItem`, repeating, with
`new from window` near 0 or no `encoded` lines while in fullscreen.

Proves it fixed (all must hold while fullscreen, over at least 30 s):

- No `capture source closed` and no `Failed to convert item` after the
  fullscreen attach.
- `encoded ~58 fps (N new from window ...)` with N > about 200 per 5 s while
  something is moving on screen (the counter, not host fps, is the proof).
- Viewer shows moving video (play the game, do not stay on a static menu).
- Exit fullscreen: window capture re-attaches to 3609304-style HWND within
  about 2 s, `new from window` recovers, no process restart storm in the host
  log (`Windows capture client lost` at most once per toggle).
- With option A: a log line stating the switch (monitor and back), and no
  other windows visible in the stream beyond what is expected.

## Workaround for players now

Do not press F11 in PCSX2 while streaming. If fullscreen is wanted, use a
borderless-windowed or maximized window instead (PCSX2 window maximized and
without F11), which keeps the same HWND. If the stream freezes after F11,
press F11 again to leave fullscreen; capture re-attaches to the windowed
window by itself (cap4 shows this works). Caveat: "borderless windowed keeps
the same HWND" is expected but not tested here. UNKNOWN until verified.

## Evidence gaps

- No polling-fast window trace during F11 (H1 unresolved).
- No WGC `Closed` reason, HRESULT, or `IsWindow` state at close (H2 unresolved).
- No key-press timestamps, so player action versus log timing is inferred.
- Only PCSX2/Qt on one machine at 2560x1600; no RPCS3 or other emulator data.
- No test of the workaround or of any fix option.
