#!/usr/bin/env bash
set -euo pipefail
echo "PCSX2: Settings → Controllers → Port 2 → SDL → DualSense Wireless Controller"
echo "Ensure couchlink-host is running so the uinput Bluetooth DualSense exists."
if [[ -d /dev/input ]]; then
  ls -l /dev/input/by-id 2>/dev/null | grep -i dualsense || true
fi
