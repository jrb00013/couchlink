#!/usr/bin/env bash
# Install and start the Windows-native stall watcher (survives WSL interop failures). Idempotent.
#   tools/rpcs3-debug/start-windows-watcher.sh [--stop]
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
WIN_DIR_WSL="/mnt/c/Users/$USER/rpcs3-tools"
mkdir -p "$WIN_DIR_WSL"
cp -f "$HERE/rpcs3-stall-watch.ps1" "$HERE/threads.ps1" "$HERE/screenshot.ps1" "$WIN_DIR_WSL/"
powershell.exe -NoProfile -Command "Get-CimInstance Win32_Process | ? { \$_.CommandLine -match 'rpcs3-stall-watch\.ps1' -and \$_.ProcessId -ne \$PID } | % { Stop-Process -Id \$_.ProcessId -Force }" 2>&1 | tr -d '\r' | head -2
[ "${1:-}" = "--stop" ] && { echo stopped; exit 0; }
powershell.exe -NoProfile -Command "Start-Process powershell -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','C:\\Users\\$USER\\rpcs3-tools\\rpcs3-stall-watch.ps1' -RedirectStandardOutput 'C:\\Users\\$USER\\rpcs3-tools\\watch.out' -RedirectStandardError 'C:\\Users\\$USER\\rpcs3-tools\\watch.err'" 2>&1 | tr -d '\r' | head -2
sleep 8; tr -d '\r' < "$WIN_DIR_WSL/watch.out" 2>/dev/null | tail -2
echo "evidence -> C:\\Users\\$USER\\rpcs3-stalls\\  (Windows side; viewable from WSL at /mnt/c/Users/$USER/rpcs3-stalls)"
