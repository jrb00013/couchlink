# Emulator binding

## Linux host

The virtual device appears as:

```
Name:    DualSense Wireless Controller
Bus:     Bluetooth
Vendor:  054c
Product: 0ce6
```

## Windows host

Install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) once (admin). Auto order:

1. DualSense VHID pipe (`\\.\pipe\couchlink-ds-vhid`) if the companion driver is running
2. ViGEm DualShock 4
3. ViGEm Xbox 360

Set `COUCHLINK_VIRTUAL_PAD=ds4` or `xbox360` to force a backend. Bind the matching
device in the emulator (DS4 / Xbox 360 / DualSense).

## RPCS3

1. Start `couchlink-host` so the virtual pad exists.
2. Open RPCS3 → Pads.
3. Assign Player 2 (or 1) to the DualSense entry that shows as Bluetooth (Linux),
   or the ViGEm DS4/Xbox pad (Windows).
4. Or run `adapters/rpcs3/configure_virtual_pad.sh` after generating a template.

Inspired by dualsensekit `scripts/windows/rpcs3_configure_pads.ps1`.

## PCSX2

1. Settings → Controllers → Controller Port 2.
2. Select SDL / DualShock → pick **DualSense Wireless Controller** (Linux) or the
   ViGEm device (Windows).
3. Helper: `adapters/pcsx2/configure_virtual_pad.sh`.

## Tips

- Create the virtual pad **before** launching the emulator so hotplug is clean.
- Local physical DualSense = Player 1; couchlink virtual = Player 2.
