#!/usr/bin/env bash
# From WSL: build (if needed) + launch couchlink-win-capture on Windows.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

is_wsl() {
  grep -qi microsoft /proc/version 2>/dev/null
}

spec="${COUCHLINK_WINDOWS_CAPTURE:-}"
if [[ -z "$spec" ]]; then
  if is_wsl; then
    spec="auto"
  else
    exit 0
  fi
fi
case "$spec" in
  0|false|local|off) exit 0 ;;
esac

if ! is_wsl; then
  echo "==> not WSL — start couchlink-win-capture manually if needed"
  exit 0
fi

if ! command -v powershell.exe >/dev/null 2>&1; then
  echo "error: powershell.exe not found — run scripts/start-win-capture.ps1 on Windows" >&2
  exit 1
fi

connect="${COUCHLINK_WIN_CAPTURE_CONNECT:-}"
if [[ -z "$connect" ]]; then
  # Prefer the WSL eth0 address. Windows 127.0.0.1:9876 often hits a stuck
  # wslrelay half-connection and never reaches the Linux listener.
  wsl_ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
  if [[ -n "$wsl_ip" ]]; then
    connect="${wsl_ip}:9876"
  else
    connect="127.0.0.1:9876"
  fi
fi
# Send frames at the stream resolution: the WSL virtual NIC, not the encoder,
# is what caps the frame rate when raw BGRA crosses it.
case "${COUCHLINK_PRESET:-720p30}" in
  1080p*) wire_w=1920; wire_h=1080 ;;
  *)      wire_w=1280; wire_h=720 ;;
esac
source_mode="${COUCHLINK_CAPTURE_SOURCE:-picker}"
window_title="${COUCHLINK_CAPTURE_WINDOW:-}"
if [[ -n "$window_title" ]]; then
  source_mode="window"
fi

build_ps1="$(wslpath -w "$ROOT/scripts/build-win-capture.ps1")"
start_ps1="$(wslpath -w "$ROOT/scripts/start-win-capture.ps1")"

echo "==> ensuring Windows capture binary is built…"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$build_ps1"

if command -v taskkill.exe >/dev/null 2>&1; then
  taskkill.exe /IM couchlink-win-capture.exe /F >/dev/null 2>&1 || true
fi

style=Minimized
[[ "$source_mode" == "picker" ]] && style=Normal

echo "==> starting Windows capture (source=$source_mode → $connect)"
# Build ArgumentList in PowerShell so quoting stays correct.
powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "
  \$argList = @('-NoProfile','-ExecutionPolicy','Bypass','-File','$start_ps1','-Connect','$connect','-Source','$source_mode','-MaxWidth','$wire_w','-MaxHeight','$wire_h')
  if ('$window_title' -ne '') { \$argList += @('-Window','$window_title') }
  Start-Process -WindowStyle $style powershell.exe -ArgumentList \$argList
" >/dev/null

echo "==> Windows capture launched (choose a window in the picker if it appears)"
exit 0
