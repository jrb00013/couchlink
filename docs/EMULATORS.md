# Emulator binding

## Player roles

| Pad | Role |
|-----|------|
| Physical DualSense / Xbox on the **host** | Player 1 — bind in RPCS3/PCSX2 directly (couchlink does not touch it) |
| Couchlink **virtual** pad | Player 2 — remote friend's controller (bound automatically, see below) |

## Controller auto-detection

The browser Gamepad API normalises every pad, so an Xbox controller and a
DualSense produce byte-identical `PadFrame`s — the host cannot tell them apart
from input alone. The player therefore announces its family over signaling:

```
player  --  pad_info { kind, id }  -->  host
```

`kind` comes from the web client's `controllerKind()` (`xbox` / `dualsense` /
`generic`). On a change the host restarts the companion with the matching
backend and rebinds the emulator slot:

| Reported kind | Companion backend | RPCS3 handler |
|---------------|-------------------|---------------|
| `xbox` | `xbox360` | XInput |
| `dualsense` | `ds4` | SDL |
| `generic` | `xbox360` | XInput |

`generic` maps to Xbox deliberately: XInput is the one handler present on every
Windows emulator build without a vendor driver.

## Automatic P2 binding

`scripts/link-emulator-pad.sh` runs from `start-host.sh` and points RPCS3's
Player 2 at the couchlink virtual pad. This exists because the failure it
prevents is invisible: RPCS3 keeps whatever device was plugged in when its
config was written — often a second DualSense that is long gone — so the
friend connects, the pad datachannel opens, and every button is silently
dropped with no error on either side.

The default `xbox360` backend enumerates through XInput, while the host's own
DualSense uses the SDL handler, so the two never collide.

| Variable | Purpose |
|----------|---------|
| `COUCHLINK_RPCS3_CONFIG` | Path to `input_configs/global/Default.yml` |
| `COUCHLINK_EMU_PLAYER` | Player slot to bind (default `2`) |
| `COUCHLINK_EMU_HANDLER` / `COUCHLINK_EMU_DEVICE` | Override the detected pair |

The original file is saved once as `Default.yml.couchlink.bak`, and the edit is
idempotent and scoped to the chosen player — Player 1 is never modified.

## Linux host

The virtual device appears as:

```
Name:    DualSense Wireless Controller
Bus:     Bluetooth / USB (uhid)
Vendor:  054c
Product: 0ce6
```

Auto order: DualSense VHID companion (WSL → Windows) → `/dev/uhid` DualSense → `uinput` DualSense.

On **WSL**, prefer the Windows DualSense VHID companion so native Windows emulators see P2:

1. On Windows: install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases), optionally [WinUHid](https://github.com/cgutman/WinUHid) for true DualSense + adaptive triggers, then run [`scripts/windows/run-ds-vhid.ps1`](../scripts/windows/run-ds-vhid.ps1) / `couchlink-ds-vhid.exe`
2. In WSL: start `couchlink-host` — Auto uses TCP `127.0.0.1:39251` when under WSL

Force with `COUCHLINK_DS_VHID=tcp` or `COUCHLINK_VIRTUAL_PAD=dualsense`.

Companion backends (`--backend` / `COUCHLINK_DS_VHID_BACKEND`):

| Backend | Virtual P2 | Friend feedback |
|---------|------------|-----------------|
| `auto` (default) | WinUHid DualSense if DLL present, else ViGEm DS4 | AT/rumble/lightbar when WinUHid |
| `winuhid` | True DualSense `054c:0ce6` | Full (rumble, lightbar, adaptive triggers) |
| `ds4` | ViGEm DualShock 4 | Limited |
| `xbox360` | ViGEm Xbox 360 | Rumble via ViGEm notifications → DSVO |

## Windows host

1. Install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) once (admin).
2. Optional: install [WinUHid](https://github.com/cgutman/WinUHid) so `WinUHidDevs.dll` is available for true DualSense P2.
3. Run **`couchlink-ds-vhid`** (companion) so Auto can use DualSense VHID over TCP/pipe.
4. Fallback order without companion: ViGEm DualShock 4 → Xbox 360.

Set `COUCHLINK_VIRTUAL_PAD=ds4` or `xbox360` to force a host-side backend. Bind the matching
device in the emulator (DS4 / Xbox 360 / DualSense).

Game rumble / adaptive-trigger **output** is forwarded to the friend when the
companion emits DSVO frames (`winuhid` or `xbox360` rumble).
## RPCS3

1. Start `couchlink-ds-vhid` (Windows) and/or `couchlink-host` so the virtual pad exists.
2. Open RPCS3 → Pads.
3. Assign Player 1 to your local DualSense; Player 2 to the ViGEm/VHID pad.
4. Or run `adapters/rpcs3/configure_virtual_pad.sh` after generating a template.

Inspired by dualsensekit `scripts/windows/rpcs3_configure_pads.ps1`.

## PCSX2

1. Settings → Controllers → Controller Port 1 = local pad; Port 2 = couchlink virtual.
2. Select SDL / DualShock → pick the ViGEm or DualSense Wireless Controller entry.
3. Helper: `adapters/pcsx2/configure_virtual_pad.sh`.

## Tips

- Create the virtual pad **before** launching the emulator so hotplug is clean.
- Local physical DualSense = Player 1; couchlink virtual = Player 2.
