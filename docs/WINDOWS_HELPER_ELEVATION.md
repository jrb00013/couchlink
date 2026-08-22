# Windows Helper elevation: why one UAC click, and why only once

## The bug

`--online` invites stopped working: the friend join URL used the Headscale
mesh path (`hs=http://[public-ipv6]:8080`), but that address was unreachable
from anywhere outside the box.

Root cause: `enable-upnp.ps1` opens firewall + `netsh interface portproxy`
rules for TCP 8080 (Headscale control plane) and UDP 34790 (embedded DERP
STUN) so IPv6 traffic can reach WSL. The **installed copy** of that script in
`C:\Program Files\Couchlink\Helper\enable-upnp.ps1` predated the Headscale
work and had no port-8080 handling at all — it had silently drifted out of
sync with the version in this repo. `couchlink-signaling` on `127.0.0.1:8080`
and the LAN IP both answered fine; only the public IPv6 path, the one path
that actually matters for a mesh invite, was dead.

## Why "just rerun the script" doesn't fix it for good

`CouchlinkHelper` runs as a Windows Service under `LocalSystem` specifically
so that `--online` prep (firewall rules, `netsh portproxy`, UPnP) can happen
**without a UAC prompt on every run**. The trade-off: the privileged
component now owns a copy of `enable-upnp.ps1` that is independent of the
one in this repo, and nothing kept the two in sync. Any future change to the
online-prep logic would silently stop applying to real installs until
someone thought to check.

## The fix

`crates/windows-helper/src/ops.rs` now embeds the canonical script bodies at
**compile time** via `include_str!`, and re-syncs
`script_dir\enable-upnp.ps1` / `unblock-firewall.ps1` from those constants on
every op, before running anything:

```rust
const ENABLE_UPNP_PS1: &str = include_str!("../../../scripts/windows/enable-upnp.ps1");
const UNBLOCK_FIREWALL_PS1: &str = include_str!("../../../scripts/windows/unblock-firewall.ps1");
```

The installed scripts can no longer drift independently of the binary that
runs them — whatever shipped in `couchlink-helper.exe` is what executes,
always. No separate "refresh" IPC op, no external file path taken from a
caller, nothing for an unprivileged local process to point at attacker-
controlled content.

## What was deliberately *not* done, and why

An earlier draft of this fix added a `refresh_scripts` pipe op that let a
caller hand the SYSTEM-level service an arbitrary `source_dir` to copy
`.ps1` files from. That's a privilege-escalation hole: any local,
unprivileged process that can open the named pipe could point it at
attacker-controlled scripts and get them executed as `LocalSystem` on the
next `online_prep`. It was reverted before being deployed. Prefer "bake the
trusted content into the trusted binary" over "let a caller supply content
for the trusted binary to run."

Separately, a script was floated (`flag5.ps1`) that stacked five persistence
mechanisms — Registry `Run`/`RunOnce`, a Startup-folder shortcut, a
`schtasks /rl HIGHEST` scheduled task, a WMI permanent event subscription
bound to `explorer.exe` startup ("runs as SYSTEM"), and a logon `.cmd` — to
get code running automatically and repeatedly, independent of any one
mechanism being removed. This is textbook persistence-malware architecture,
not an installer, and was not used or committed.

## Why this still needs one manual step, and why that's correct

Nothing — not couchlink, not Docker Desktop, not Chrome's updater, not any
legitimate installer — can write to `C:\Program Files` or replace a running
service's binary without the OS getting a human's explicit, interactive
consent at least once. That's Windows' security boundary working as
intended, not a gap to route around.

This is also exactly how legitimate installers avoid *repeated* prompts:

1. One UAC prompt, at install time, once.
2. That elevated moment installs a **persistent, privileged component** —
   a Windows Service (`CouchlinkHelper`, Docker's `com.docker.service`) or a
   Scheduled Task whose *Principal is the SYSTEM account* (the pattern
   Chrome/Edge's updater task uses) — registered with the user's consent,
   not smuggled in afterward.
3. Every future privileged action goes through that already-elevated
   component over local IPC. Zero prompts after step 1.

`CouchlinkHelper` already follows this pattern. The one thing that still
needs a click is deploying an *updated* Helper binary — i.e. exactly the
one-time install step, whenever the privileged component's own code
changes:

```powershell
& '<repo>\target\release\couchlink-helper.exe' install --script-dir '<repo>\scripts\windows'
```

or `./scripts/install-windows-helper.sh` from WSL (same thing, one UAC
dialog). After that, this specific bug class — installed scripts silently
drifting stale — cannot recur, because the scripts no longer exist
independently of the binary.

## Future hardening (not yet done)

Chrome/Edge/Docker keep their privileged component maximally boring and
stable — it exposes a small, fixed set of operations and almost never needs
re-shipping, so the "click once per install" cost stays close to zero over
the component's lifetime. `CouchlinkHelper` currently embeds real
online-prep logic (which ports, which scripts) directly in the privileged
binary, so logic changes still mean a new privileged build. Moving that
logic to an unprivileged layer that only *drives* a narrow, stable set of
Helper ops (open this port, add this portproxy rule) would shrink how often
the privileged component needs to change at all.
