# Emulator binding

## Player roles

| Pad | Role |
|-----|------|
| Physical DualSense / Xbox on the **host** | Player 1 — bind in RPCS3/PCSX2 directly (couchlink does not touch it) |
| Couchlink **virtual** pad | Player 2 — remote friend's controller |

## Linux host

The virtual device appears as:

```
Name:    DualSense Wireless Controller
Bus:     Bluetooth
Vendor:  054c
Product: 0ce6
```

On **WSL**, prefer the Windows DualSense VHID companion so native Windows emulators see P2:

1. On Windows: install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases), then run `couchlink-ds-vhid.exe`
2. In WSL: start `couchlink-host` — Auto uses TCP `127.0.0.1:39251` when under WSL

Force with `COUCHLINK_DS_VHID=tcp` or `COUCHLINK_VIRTUAL_PAD=dualsense`.

## Windows host

1. Install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) once (admin).
2. Run **`couchlink-ds-vhid`** (companion) so Auto can use DualSense VHID over TCP/pipe.
3. Fallback order without companion: ViGEm DualShock 4 → Xbox 360.

Set `COUCHLINK_VIRTUAL_PAD=ds4` or `xbox360` to force a backend. Bind the matching
device in the emulator (DS4 / Xbox 360 / DualSense).

Game rumble / adaptive-trigger **output** is forwarded to the friend when the
companion emits DSVO frames (WinUHid DualSense backend). ViGEm DS4 injects
input today; full adaptive-trigger capture needs WinUHid.

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
