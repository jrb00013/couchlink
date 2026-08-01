#!/usr/bin/env bash
# Prepare Windows for couchlink --online (Private profile, discovery, firewall,
# WSL portproxy IPv4+IPv6, NATUPnP maps). Prefer Task Scheduler
# "CouchlinkElevatedUpnp" (no UAC after first approve).
#
# Usage: ./scripts/enable-upnp.sh [--skip-map]
# Exit: 0 = ready/mapped, 2 = Windows OK but router IGD still off, else error
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

SKIP_MAP=0
for a in "$@"; do
  case "$a" in
    --skip-map) SKIP_MAP=1 ;;
    -h|--help)
      echo "usage: $0 [--skip-map]"
      exit 0
      ;;
  esac
done

ps_win="$(command -v powershell.exe 2>/dev/null || true)"
[[ -n "${ps_win:-}" ]] || { echo "powershell.exe not found (WSL/Windows required)" >&2; exit 1; }

# Always elevate with the real Windows PowerShell path — Start-Process breaks on /mnt/c/...
PS_WIN_EXE='C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'
if [[ -x /mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe ]]; then
  PS_WIN_LAUNCH='/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe'
else
  PS_WIN_LAUNCH="$ps_win"
fi

WIN_USER=""
if command -v cmd.exe >/dev/null 2>&1; then
  WIN_USER="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r')"
fi
WIN_USER="${WIN_USER:-josep}"
RUN_LINUX="/mnt/c/Users/${WIN_USER}/AppData/Local/couchlink-run"
MARKER="$RUN_LINUX/enable-upnp.exit"
mkdir -p "$RUN_LINUX"
cp -f "$ROOT/scripts/windows/enable-upnp.ps1" "$RUN_LINUX/enable-upnp.ps1"
script_w="$(wslpath -w "$RUN_LINUX/enable-upnp.ps1")"
rm -f "$MARKER"

WSL_IP="$(ip -4 -o addr show eth0 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | head -1 || true)"
WSL_IP="${WSL_IP:-$(hostname -I 2>/dev/null | awk '{print $1}')}"

SCHTASKS="/mnt/c/Windows/System32/schtasks.exe"
TASK_NAME="CouchlinkElevatedUpnp"
used_task=0

wait_marker() {
  local waited=0
  while [[ ! -f "$MARKER" && $waited -lt 90 ]]; do
    sleep 1
    waited=$((waited + 1))
  done
  if [[ ! -f "$MARKER" ]]; then
    return 1
  fi
  tr -d '\r\n' <"$MARKER"
  return 0
}

if [[ -x "$SCHTASKS" ]] && "$SCHTASKS" /Query /TN "$TASK_NAME" &>/dev/null; then
  echo "==> Windows online prep via saved task (no UAC)"
  if "$SCHTASKS" /Run /TN "$TASK_NAME" &>/dev/null; then
    used_task=1
    ec="$(wait_marker || true)"
    if [[ -z "${ec:-}" ]]; then
      echo "==> saved task did not finish — falling back to UAC" >&2
      used_task=0
    fi
  else
    echo "==> saved task failed to start — falling back to UAC" >&2
  fi
fi

if [[ "$used_task" -eq 0 ]]; then
  echo "==> elevating enable-upnp.ps1 (approve UAC once; later --online skips this)"
  set +e
  # Keep ArgumentList simple — complex quoting breaks Start-Process under WSL.
  if [[ "$SKIP_MAP" == "1" ]]; then
    "$PS_WIN_LAUNCH" -NoProfile -Command \
      "\$p = Start-Process -FilePath '$PS_WIN_EXE' -Verb RunAs -PassThru -Wait -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$script_w','-SkipMap','-WslIp','${WSL_IP:-}'); if (\$null -eq \$p) { exit 1 }; exit \$p.ExitCode"
  else
    "$PS_WIN_LAUNCH" -NoProfile -Command \
      "\$p = Start-Process -FilePath '$PS_WIN_EXE' -Verb RunAs -PassThru -Wait -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$script_w','-WslIp','${WSL_IP:-}'); if (\$null -eq \$p) { exit 1 }; exit \$p.ExitCode"
  fi
  ec=$?
  set -e
  if [[ -f "$MARKER" ]]; then
    ec="$(tr -d '\r\n' <"$MARKER")"
  fi
fi

ec="${ec:-1}"
case "$ec" in
  0) echo "==> Windows online prep OK (UPnP maps applied)" ;;
  2) echo "==> Windows prepared (firewall/portproxy); router UPnP still off — using IPv6/tunnel fallback if needed" ;;
  *) echo "==> enable-upnp exited $ec" >&2 ;;
esac
exit "$ec"
