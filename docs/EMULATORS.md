# Emulator binding

The host virtual device appears as:

```
Name:    DualSense Wireless Controller
Bus:     Bluetooth
Vendor:  054c
Product: 0ce6
```

## RPCS3

1. Start `couchlink-host` so the virtual pad exists.
2. Open RPCS3 → Pads.
3. Assign Player 2 (or 1) to the DualSense entry that shows as Bluetooth.
4. Or run `adapters/rpcs3/configure_virtual_pad.sh` after generating a template.

Inspired by dualsensekit `scripts/windows/rpcs3_configure_pads.ps1`.

## PCSX2

1. Settings → Controllers → Controller Port 2.
2. Select SDL / DualShock → pick **DualSense Wireless Controller**.
3. Helper: `adapters/pcsx2/configure_virtual_pad.sh`.

## Tips

- Create the virtual pad **before** launching the emulator so hotplug is clean.
- Local physical DualSense = Player 1; couchlink virtual = Player 2.
