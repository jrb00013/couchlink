# Input lock (block viewer input on the host)

`scripts/input-lock.sh on|off|toggle|status`, or press `l` in `couchlink-tui` (the control panel).
While locked the host replaces every viewer pad frame with a neutral one (so held buttons are released too);
viewers keep seeing video but cannot press anything. It takes effect within ~150 ms with no restart.
Keyboard/mouse from the browser are mapped onto the same virtual pad, so they are blocked as well.

The lock is a file: `COUCHLINK_INPUT_LOCK_FILE`, else `%LOCALAPPDATA%\couchlink\input.lock` (Windows host),
else `${XDG_RUNTIME_DIR:-/tmp}/couchlink-input.lock`. Present = locked. The host logs
`input lock ENGAGED` / `input lock released` on every change.

## Why it exists
Viewer input is injected as a virtual game controller. To Windows that is a real controller, and it goes to
whatever window is in the foreground. Reported symptom: with a friend playing, switching the host to another
virtual desktop and clicking in the terminal made the terminal's tab strip highlight and then close every tab.
Likely mechanism (not reproduced, so unconfirmed): XAML/WinUI apps such as Windows Terminal handle gamepad
D-pad/A/B focus navigation, so controller presses move focus across the tab strip and activate close buttons.
Microsoft documents that a controller only reaches the focused window, and that Windows maps gamepad
buttons to focus navigation (https://learn.microsoft.com/windows/apps/develop/input/gamepad-and-remote-interactions).

Use the lock before working in another window while friends are connected. A possible follow-up is locking
automatically whenever the captured window is not the foreground window.

Co-developed with David Poindexter (@Dpoin23), Joel S. (@neutral207) and Ricardo Beale (@RiccoWrld).

## Automatic focus gate (Windows companion)
`couchlink-ds-vhid` neutralises every viewer report while the host's foreground window is a terminal or shell
(default: WindowsTerminal, wt, OpenConsole, conhost, cmd, powershell, pwsh, wsl, wslhost, explorer), so the
tab-closing case cannot happen even without the manual lock. It logs `viewer input BLOCKED (foreground = ...)`
and `live again`.
- `COUCHLINK_INPUT_BLOCK_FOREGROUND=a.exe,b.exe` replaces the block list; `off` disables the gate.
- `COUCHLINK_INPUT_ONLY_FOREGROUND=rpcs3.exe` is strict mode: pad input only while that exe is in front.
Takes effect when the companion is restarted (restarting it re-plugs the virtual pads).
