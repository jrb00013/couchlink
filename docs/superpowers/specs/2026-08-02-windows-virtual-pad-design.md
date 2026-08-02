# Windows virtual DualSense / Xbox / DS4 host pad — Design

**Date:** 2026-08-02  
**Status:** Approved for implementation (user: custom DualSense VHID + Xbox + PS4)  
**Branch:** `feat/windows-virtual-dualsense-pad`

## Problem

Native Windows hosts cannot inject a pad today (`VirtualPad::create` bails). WSL+uinput works but pure Windows emulator users need a host-side virtual controller.

## Goals

1. **Windows host** injects pad state from `PadFrame` into a virtual device emulators see.
2. **Primary identity:** Custom **DualSense VHID** (`054c:0ce6`) via a companion driver/pipe when available.
3. **Fallbacks (ship now):** Nefarius **ViGEmBus** — Xbox 360 and DualShock 4 targets (widely supported by PCSX2/RPCS3/Windows games).
4. **Client capture:** Native support for DualSense, **DualShock 4 (PS4)**, and **Xbox** families (web already covers Standard Gamepad).

## Non-goals (this PR)

- Shipping a signed kernel DualSense driver binary in-tree (interface + protocol only; driver is a separate artifact).
- Adaptive triggers / full output-report passthrough (separate branch/PR).
- Replacing Linux uinput path.

## Architecture

```
PadFrame (CLPD)
    │
    ├─ Linux: uinput DualSense BT (existing)
    └─ Windows Auto:
           1. DualSense VHID pipe/driver if present
           2. else ViGEm DualShock 4
           3. else ViGEm Xbox 360
```

Env override: `COUCHLINK_VIRTUAL_PAD=auto|dualsense|ds4|xbox360|noop`

### DualSense VHID protocol (userspace ↔ driver)

Named pipe: `\\.\pipe\couchlink-ds-vhid`

JSON line or binary frame (v1 binary preferred):

- Magic `DSVH` + version + packed DualSense USB-style input report (64 bytes, report id `0x01`) derived from `PadFrame`.

If the pipe is missing, Auto falls through to ViGEm.

### ViGEm

Requires [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) installed once (UAC). Uses `vigem-client` Rust crate.

## Client PS4 / Xbox

- Extend native Linux hidraw accept list + DualShock 4 report parser → `PadFrame`.
- Windows client: keep keyboard; XInput/hid path can follow (web Gamepad already works for Xbox/PS4/DualSense).

## Success

- Windows host with ViGEmBus: virtual Xbox 360 or DS4 updates from pad channel.
- With DualSense VHID companion connected: Auto selects DualSense path.
- Linux DualShock 4 hidraw parses to same `PadFrame` as DualSense/Xbox.
- `cargo test -p couchlink-pad` passes on Linux CI.
