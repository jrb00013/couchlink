#!/usr/bin/env bash
# Point humans at the virtual Bluetooth DualSense created by couchlink-host.
# For two local Windows DualSenses fighting over the same RPCS3 slot, use:
#   powershell.exe -ExecutionPolicy Bypass -File adapters/rpcs3/configure_local_pads.ps1
set -euo pipefail
echo "Couchlink virtual pad should appear as:"
echo "  DualSense Wireless Controller (Bluetooth, 054c:0ce6)"
echo
if command -v python3 >/dev/null; then
  python3 - <<'PY'
import glob, os
for path in sorted(glob.glob('/sys/class/input/js*/device/name')):
    try:
        name = open(path).read().strip()
    except OSError:
        continue
    if 'DualSense' in name or 'Wireless Controller' in name:
        print(f"found: {name} ({path})")
PY
fi
echo
echo "In RPCS3 → Pads, bind Player 2 to that device."
echo "Windows local two-pad fix: adapters/rpcs3/configure_local_pads.ps1 (SDL handler)."
