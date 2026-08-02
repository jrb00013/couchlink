# DualSense VHID companion + game output passthrough — Design

**Date:** 2026-08-02  
**Status:** Implemented on `feat/dualsense-vhid-companion` (ViGEm + optional WinUHid; Linux UHID)  
**Branch:** `feat/dualsense-vhid-companion`

## Roles (important)

| Pad | Who | Couchlink role |
|-----|-----|----------------|
| Physical DualSense on **host** | Local player (P1) | **None** — RPCS3/PCSX2 bind it directly |
| Virtual DualSense on **host** | Remote friend (P2) | Inject CLPD → virtual device; forward game HID **output** → friend |
| Physical DualSense on **client** | Friend | Capture input; apply rumble / adaptive triggers / lightbar |

## Goals

1. True DualSense identity (`054c:0ce6`) for P2 when possible (not only ViGEm DS4/Xbox).
2. Game → virtual pad **output reports** (rumble, lightbar, adaptive triggers) auto-forwarded to the friend via existing `PadFeedback` / `raw_output`.
3. Works for **native Windows host** and **WSL host** driving a Windows-side virtual device (emulators on Windows).

## Architecture

```
Friend DualSense ──CLPD──► couchlink-host ──DSVH──► companion ──► Virtual DualSense (P2)
                              ▲                         │
                              │         game HID OUT    │
                              └──── DSVO / feedback ◄───┘
                                         │
                                         ▼
                              PadFeedback JSON ──► friend DualSense
```

### Transport

| Host runs on | Path to companion |
|--------------|-------------------|
| Native Windows | `\\.\pipe\couchlink-ds-vhid` |
| WSL2 | TCP `127.0.0.1:39251` (companion listens; WSL reaches Windows localhost) |

Framing (both directions):

- Host→companion input: `DSVH` + `u8 ver=1` + 64-byte DualSense USB input report  
- Companion→host output: `DSVO` + `u8 ver=1` + `u16 le len` + raw HID output bytes  

### Companion backends (Windows)

1. **WinUHid DualSense** when `WinUHid` / `WinUHidDevs` is installed (preferred true `054c:0ce6`).
2. Else **ViGEm DualShock 4** inside the companion (still gets rumble notifications; not full adaptive triggers).
3. Install docs: one-time driver MSI + `couchlink-ds-vhid.exe` (can run via existing Helper / Startup).

### Linux / WSL emulators (optional path)

When the emulator itself runs under Linux, host uses **`/dev/uhid` DualSense** so `hid-playstation` binds and delivers OUTPUT reports without the Windows companion. Auto order on Linux: UHID → existing uinput fallback.

## Non-goals

- Capturing or hiding the host’s physical DualSense (P1).
- EV-signed kernel driver authored in-tree (we consume WinUHid / ViGEm).
- Browser Gamepad vibration.

## Success

- Windows + companion: PCSX2/RPCS3 see a DualSense (or DS4 fallback) for P2; friend input moves it.
- Game rumble/adaptive trigger output reaches friend’s DualSense when backend supports it.
- WSL host can drive the Windows companion over TCP without named-pipe hacks.
- `cargo test --workspace` green.
