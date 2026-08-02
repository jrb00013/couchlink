# Windows privileged helper (zero UAC on run) — Design

**Date:** 2026-08-02  
**Status:** Approved (approach A — service + elevated host installer)  
**Branch:** `feat/windows-privileged-helper`

## Problem

On WSL/Windows, `--online` still needs elevation for firewall rules, WSL `portproxy`, and UPnP prep. Today that is either:

- interactive UAC (`Start-Process -Verb RunAs`), or
- a one-time UAC that registers `CouchlinkElevatedUpnp` / `CouchlinkElevatedWireGuard` Scheduled Tasks.

We want **option 1**: after a real installer (or silent enterprise deploy), `./scripts/run.sh host --online` and `--unblock-firewall` **never** show UAC — including the first run after install.

## Non-goals (this pass)

- Removing the need for **any** elevation ever (the installer itself will elevate once, by design).
- Shipping an MSI/Intune package in v1 (design allows it; Inno Setup first).
- Replacing Headscale / Tailscale client install logic (already avoided for Headscale).
- Elevating `couchlink-win-capture` or game capture (not required today).
- macOS/Linux privileged helpers (Linux already has uinput helper; out of scope).

## Goals

1. **Elevated install once** → Windows service running as LocalSystem.
2. **Non-elevated callers** (WSL bash via `powershell.exe`, or native Windows scripts) ask the service to perform privileged ops.
3. **Same outcomes** as today’s `enable-upnp.ps1` / `unblock-firewall.ps1` (firewall, portproxy, network discovery, NATUPnP maps).
4. **Safe default:** random processes must not open arbitrary ports or rewrite firewall; pipe ACL + allowlisted ops only.
5. **Graceful degrade:** if the service is missing, print “install Couchlink Host Helper” — do **not** auto-pop UAC unless `COUCHLINK_ALLOW_UAC=1` (escape hatch for developers).

## Architecture

```
WSL / run.sh                    Windows (user)                 Windows (SYSTEM)
-------------                   --------------                 -----------------
enable-upnp.sh ──► call-helper.ps1 ──named pipe──► couchlink-helper.exe (service)
unblock-firewall.sh                 │                              │
                                    │                              ├─ firewall rules
                                    │                              ├─ netsh portproxy
                                    │                              └─ UPnP / discovery
                                    ▼
                         (no UAC — pipe client is
                          normal user PowerShell)
```

### Components

| Component | Role |
|-----------|------|
| `couchlink-helper.exe` | Windows service + CLI (`install` / `uninstall` / `run` / `ping`) |
| Named pipe `\\.\pipe\couchlink-helper` | JSON-lines request/response |
| `scripts/windows/call-helper.ps1` | Non-elevated pipe client |
| `scripts/lib-windows-helper.sh` | WSL/bash: prefer helper; optional UAC fallback |
| `CouchlinkHelper-Setup.exe` | Inno Setup, `PrivilegesRequired=admin` — installs service |
| Existing `enable-upnp.ps1` / `unblock-firewall.ps1` | Reused by the service (shipped under install dir) |

### Why a service (not only Scheduled Tasks)

Tasks already give “no UAC after first click,” but:

- first run still needs UAC or a missing-task fallback,
- args/markers are racey,
- enterprise wants a real product surface (service + installer + uninstall).

Scheduled Tasks remain a **legacy fallback** when the service is absent and `COUCHLINK_ALLOW_UAC=1`.

## Wire protocol (JSON lines)

One request object per line; one response object per line. UTF-8.

### Requests

```json
{"op":"ping"}
{"op":"online_prep","skip_map":true,"wsl_ip":"172.18.0.2","signaling_port":8443,"turn_port":3478}
{"op":"firewall_unblock"}
```

`online_prep` maps to today’s `enable-upnp.ps1` (Private profile, discovery, firewall 8443/3478 + Headscale 8080/34790, WSL portproxy, optional NATUPnP).  
`firewall_unblock` maps to `unblock-firewall.ps1`.

### Responses

```json
{"ok":true,"op":"ping","version":"0.1.1"}
{"ok":true,"op":"online_prep","exit":0}
{"ok":false,"op":"online_prep","exit":2,"error":"igd missing"}
{"ok":false,"error":"unknown op"}
{"ok":false,"error":"unauthorized"}
```

Exit codes for `online_prep` match `enable-upnp.ps1`: `0` = OK/mapped, `2` = Windows OK but router IGD missing, other = failure.

### Security

1. **Pipe ACL:** create pipe with discretionary ACL allowing:
   - `BUILTIN\Administrators` (full)
   - the installing user’s SID (read/write) — persisted at install in `helper-acl.txt` / registry
   - optionally `NT AUTHORITY\INTERACTIVE` for console sessions
2. **No arbitrary shell:** only allowlisted `op` values; scripts live under `%ProgramFiles%\Couchlink\Helper\` and are not path-injected by the client.
3. **No remote bind:** pipe is local-only; do not expose a LAN TCP control port in v1.
4. **WSL path:** callers always go through `powershell.exe` on the Windows side so the pipe client runs as the Windows user (pipe ACL matches).

## Installer (v1: Inno Setup)

New: `packaging/windows/couchlink-helper.iss` → `CouchlinkHelper-Setup-{version}.exe`.

| Setting | Value |
|---------|--------|
| `PrivilegesRequired` | `admin` |
| Default dir | `{commonpf}\Couchlink\Helper` |
| Files | `couchlink-helper.exe`, `enable-upnp.ps1`, `unblock-firewall.ps1` (copies) |
| Post-install | `couchlink-helper.exe install` + `Start-Service` |
| Uninstall | stop service, `couchlink-helper.exe uninstall`, remove firewall rules tagged `couchlink-*` (best-effort) |

User experience: double-click setup → Windows UAC once → service running. After that, WSL `run.sh --online` is silent.

### Enterprise (phase 2, same binary)

- Wrap the same files as MSI (WiX) for Intune/GPO silent install: `msiexec /i CouchlinkHelper.msi /qn`.
- GPO can deploy the MSI to gaming PCs; no interactive UAC for end users.
- Not required to ship in the first PR; protocol and service must not block MSI later.

## Script integration

### Preference order (`lib-windows-helper.sh`)

1. If helper responds to `ping` → use it.
2. Else if legacy Scheduled Task exists → `schtasks /Run` (no UAC).
3. Else if `COUCHLINK_ALLOW_UAC=1` → existing `-Verb RunAs` path.
4. Else print install hint and return non-zero (or soft-continue where today’s scripts already soft-fail).

### Call sites to switch

| Script | Change |
|--------|--------|
| `scripts/enable-upnp.sh` | Prefer helper `online_prep` |
| `scripts/unblock-firewall.sh` | Prefer helper `firewall_unblock` |
| `scripts/run.sh` | Messaging: “Helper service” vs UAC |
| `docs/PLAY_TOGETHER.md`, `docs/NO_COMPUTER_UX.md`, `docs/MESH.md` | Document one-time helper install |

WireGuard elevate remains separate for now (can add `op: wireguard_up` later).

## Build

- New workspace crate: `crates/windows-helper` → bin `couchlink-helper`.
- Windows-only (`cfg(windows)`); build via existing Windows cargo path (same as `couchlink-win-capture`): `packaging/windows/build-helper-installer.ps1`.
- Dependencies: `windows-service` (or equivalent), `serde`/`serde_json`, `clap`, named-pipe APIs via `windows` crate.

## Testing

| Layer | How |
|-------|-----|
| Protocol unit tests | Parse/serialize request/response on any OS |
| Pipe smoke (Windows) | `couchlink-helper run` (console mode) + `call-helper.ps1 -Op ping` |
| Integration | After install: `enable-upnp.sh --skip-map` completes with “via helper (no UAC)” |
| Negative | Stop service → clear message, no unexpected UAC |

## Success criteria

1. Fresh machine: install `CouchlinkHelper-Setup.exe` (one UAC) → service Running.
2. From WSL: `./scripts/run.sh host --online` performs firewall/portproxy **without** a UAC prompt.
3. Uninstall removes the service; subsequent helper calls fail cleanly with install hint.
4. Developers can still force old UAC path with `COUCHLINK_ALLOW_UAC=1`.
