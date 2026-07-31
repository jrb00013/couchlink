#!/usr/bin/env bash
# Prepare Windows for real router UPnP (Private profile, discovery, services, maps).
# Prefer Task Scheduler "CouchlinkElevatedUpnp" (no UAC after first approve).
# Falls back to one UAC elevate which registers that task.
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
  echo "==> Windows UPnP prep via saved task (no UAC)"
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
  if [[ "$SKIP_MAP" == "1" ]]; then
    "$ps_win" -NoProfile -Command \
      "\$p = Start-Process -FilePath '$ps_win' -Verb RunAs -PassThru -Wait -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$script_w','-SkipMap'); if (\$null -eq \$p) { exit 1 }; exit \$p.ExitCode"
  else
    "$ps_win" -NoProfile -Command \
      "\$p = Start-Process -FilePath '$ps_win' -Verb RunAs -PassThru -Wait -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$script_w'); if (\$null -eq \$p) { exit 1 }; exit \$p.ExitCode"
  fi
  ec=$?
  set -e
  if [[ -f "$MARKER" ]]; then
    ec="$(tr -d '\r\n' <"$MARKER")"
  fi
fi

ec="${ec:-1}"
case "$ec" in
  0) echo "==> UPnP ready (or maps applied)" ;;
  2) echo "==> Windows prepared; router UPnP still off — forward TCP 8443 + UDP/TCP 3478 if friends can't join" ;;
  *) echo "==> enable-upnp exited $ec" >&2 ;;
esac
exit "$ec"
