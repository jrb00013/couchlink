#!/usr/bin/env bash
# From WSL: start couchlink-win-capture on Windows if needed, then wait until :9876 accepts.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

is_wsl() {
  grep -qi microsoft /proc/version 2>/dev/null
}

windows_host_ip() {
  awk '/^nameserver / { print $2; exit }' /etc/resolv.conf 2>/dev/null
}

port_open() {
  local host="$1" port="$2"
  timeout 1 bash -c "echo >/dev/tcp/${host}/${port}" 2>/dev/null
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

if [[ "$spec" == "auto" ]]; then
  host="$(windows_host_ip)"
  [[ -n "$host" ]] || {
    echo "error: WSL auto capture needs Windows IP in /etc/resolv.conf" >&2
    exit 1
  }
  addr="${host}:9876"
else
  addr="$spec"
  host="${addr%%:*}"
  port="${addr##*:}"
  [[ "$host" != "$port" ]] || port=9876
fi
port="${port:-9876}"
host="${host:-${addr%%:*}}"

if port_open "$host" "$port"; then
  echo "==> Windows capture already listening on ${host}:${port}"
  exit 0
fi

if ! is_wsl; then
  echo "error: nothing listening on ${host}:${port} and not in WSL to auto-start win-capture" >&2
  exit 1
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

echo "==> starting Windows desktop capture (powershell → couchlink-win-capture)"
# Detach so this script can wait on the TCP port; host shutdown uses taskkill.
powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \
  "Start-Process -WindowStyle Minimized powershell.exe -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$win_script')" \
  >/dev/null 2>&1 || {
  echo "error: failed to launch start-win-capture.ps1 via powershell.exe" >&2
  exit 1
}

echo "==> waiting for Windows capture on ${host}:${port} (first run may build the Windows binary)…"
for _ in $(seq 1 180); do
  if port_open "$host" "$port"; then
    echo "==> Windows capture ready"
    exit 0
  fi
  sleep 0.5
done

echo "error: timed out waiting for ${host}:${port}" >&2
echo "  Allow inbound TCP ${port} in Windows Firewall, or run: .\\scripts\\start-win-capture.ps1" >&2
exit 1
