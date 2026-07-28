#!/usr/bin/env bash
# From WSL: launch couchlink-win-capture on Windows (outbound to WSL :9876).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

is_wsl() {
  grep -qi microsoft /proc/version 2>/dev/null
}

# COUCHLINK_WINDOWS_CAPTURE=0|false disables the bridge.
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

win_script="$(wslpath -w "$ROOT/scripts/start-win-capture.ps1" 2>/dev/null || true)"
if [[ -z "$win_script" ]]; then
  echo "error: could not map $ROOT to a Windows path" >&2
  exit 1
fi

# Prefer localhost forwarding (Windows → WSL). Fall back to eth0 IP if needed.
connect="${COUCHLINK_WIN_CAPTURE_CONNECT:-127.0.0.1:9876}"

echo "==> starting Windows desktop capture → ${connect}"
# Kill any old server-mode instance; new client reconnects until host listens.
if command -v taskkill.exe >/dev/null 2>&1; then
  taskkill.exe /IM couchlink-win-capture.exe /F >/dev/null 2>&1 || true
fi

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \
  "Start-Process -WindowStyle Minimized powershell.exe -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$win_script','-Connect','$connect')" \
  >/dev/null 2>&1 || {
  echo "error: failed to launch start-win-capture.ps1 via powershell.exe" >&2
  exit 1
}

echo "==> Windows capture client launched (host will accept on :9876)"
exit 0
