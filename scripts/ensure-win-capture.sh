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
  if [[ "${COUCHLINK_CAPTURE_TRANSPORT:-hyperv}" == "tcp" ]]; then
    # Which address Windows should dial depends on the WSL networking mode.
    #
    # NAT mode: WSL has its own private eth0 and 127.0.0.1 on the Windows side
    # often hits a stuck wslrelay half-connection, so dial the eth0 address.
    #
    # Mirrored mode: the two share a network stack, so loopback is correct — and
    # `hostname -I` is actively wrong there. It returns whichever of a dozen
    # addresses sorts first, which was 10.66.0.1 (a WireGuard interface) — not
    # routable from Windows. win-capture then never connected, the host blocked
    # forever on its first frame, and the player saw only "waiting for host".
    if ip -6 addr show scope global 2>/dev/null | grep -q 'inet6 [23]'; then
      connect="127.0.0.1:9876"
    else
      wsl_ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
      if [[ -n "$wsl_ip" ]]; then
        connect="${wsl_ip}:9876"
      else
        connect="127.0.0.1:9876"
      fi
    fi
  else
    # Default transport: a Hyper-V socket over the VMBus channel WSL2 already
    # uses internally, instead of TCP over the vEthernet/NAT hop above.
    #
    # Live-tested 2026-08-19: binding Windows' AF_HYPERV listener to the
    # documented "any partition" wildcard VmId (HV_GUID_ZERO) does NOT work
    # for WSL2's utility VM — the guest's connect times out (os error 110).
    # The fix: give win-capture the *specific* WSL VM GUID to bind to.
    # `wslinfo --vm-id` returns exactly that, with no admin rights needed
    # (unlike `hcsdiag list`, which needs Hyper-V Administrators membership).
    # Set COUCHLINK_CAPTURE_TRANSPORT=tcp to fall back to the old path.
    if ! command -v wslinfo >/dev/null 2>&1; then
      echo "warning: wslinfo not found (WSL package too old for --vm-id) — falling back to TCP capture transport" >&2
      wsl_ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
      connect="${wsl_ip:-127.0.0.1}:9876"
    else
      vm_id="$(wslinfo --vm-id 2>/dev/null)"
      if [[ -z "$vm_id" ]]; then
        echo "warning: wslinfo --vm-id returned nothing — falling back to TCP capture transport" >&2
        wsl_ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
        connect="${wsl_ip:-127.0.0.1}:9876"
      else
        connect="hyperv:9877:${vm_id}"
      fi
    fi
  fi
fi
# Send frames at the stream resolution: the WSL virtual NIC, not the encoder,
# is what caps the frame rate when raw BGRA crosses it.
case "${COUCHLINK_PRESET:-1080p60}" in
  1080p60|hd60) wire_w=1920; wire_h=1080; bitrate_kbps=18000 ;;
  1080p30|hd30) wire_w=1920; wire_h=1080; bitrate_kbps=10000 ;;
  720p60)       wire_w=1280; wire_h=720;  bitrate_kbps=10000 ;;
  *)             wire_w=1280; wire_h=720;  bitrate_kbps=5000 ;;
esac
bitrate_kbps="${COUCHLINK_BITRATE_KBPS:-$bitrate_kbps}"
# Capture/encode cadence. Measured on an RTX 5080 at 720p: 60Hz gives
# capture->encoded p50 11-13ms, 120Hz gives p50 8-9ms — the beat is half the
# wait, so halving it halves that half. The cost is double the encode and
# roughly double the bitrate's worth of frames, so it is opt-in.
capture_fps="${COUCHLINK_CAPTURE_FPS:-60}"
source_mode="${COUCHLINK_CAPTURE_SOURCE:-picker}"
window_title="${COUCHLINK_CAPTURE_WINDOW:-}"
if [[ -n "$window_title" ]]; then
  source_mode="window"
fi

build_ps1="$(wslpath -w "$ROOT/scripts/build-win-capture.ps1")"
start_ps1="$(wslpath -w "$ROOT/scripts/start-win-capture.ps1")"

# Every powershell.exe from WSL opens a *visible* conhost (the blue window)
# unless -WindowStyle Hidden is set. Host respawn calls this every 20s when
# capture is down — without Hidden that looks like "couchlink is still on."
psw() {
  powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass "$@"
}

if [[ "${COUCHLINK_SKIP_WIN_CAPTURE_BUILD:-0}" != "1" ]]; then
  echo "==> ensuring Windows capture binary is built…"
  if ! psw -File "$build_ps1" >/dev/null 2>&1; then
    echo "error: could not build couchlink-win-capture.exe" >&2
    echo "       install Rust on Windows (https://rustup.rs, MSVC toolchain), then retry: ./scripts/build-win-capture.ps1" >&2
    exit 1
  fi
fi

if command -v taskkill.exe >/dev/null 2>&1; then
  taskkill.exe /IM couchlink-win-capture.exe /F >/dev/null 2>&1 || true
fi

# The picker UI lives on the capture exe, not this wrapper. Keep the
# PowerShell host Hidden so respawn/build never flash a blue console.
# Minimized still creates a conhost that pops then minimizes.

if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
  echo "==> starting Windows capture (source=$source_mode → $connect @ ${wire_w}x${wire_h} ${bitrate_kbps}kbps)"
fi

# docs/INCIDENT-2026-08-19-terminals-died.md, root cause #1: `Start-Process`
# here spawns win-capture as a descendant of *this* WSL session's process
# tree. Windows Terminal puts every tab's whole tree in one Job Object with
# kill-on-close, so a Start-Process child does not survive the terminal
# crashing — it goes down with it, silently, and nothing ever relaunches it.
# A Scheduled Task runs from the Task Scheduler service instead: it is never
# a member of that job, so a dead terminal (crash or a closed window) cannot
# take win-capture with it. `/IT` keeps it interactive (attached to the
# logged-on session), which the picker window needs to be visible/clickable.
#
# `schtasks /TR` silently truncates around ~262 characters (verified live:
# the full args + the `\\wsl.localhost\...` UNC path to start-win-capture.ps1
# blew past it and `-BitrateKbps 18000` came out the other end as `180` — a
# 100x bitrate cut with no error from schtasks at all). Fixed by writing the
# real argument list into a short local launcher script once, in a fixed,
# always-short local path, and pointing `/TR` at *that* instead — the task's
# command line is then constant-length regardless of how many capture args
# there are.
task_name="couchlink-win-capture"
# Picker must be visible/clickable. schtasks /IT + Hidden PowerShell often never
# surfaces GraphicsCapturePicker; launching the exe (or its Normal-style wrapper)
# with Start-Process does. Keep the Scheduled Task for desktop/window so mid-
# session respawn still survives a terminal crash.
_ps_style="Hidden"
[[ "$source_mode" == "picker" ]] && _ps_style="Normal"
psw -Command "
  \$argList = @('-NoProfile','-WindowStyle','$_ps_style','-ExecutionPolicy','Bypass','-File','$start_ps1','-Connect','$connect','-Source','$source_mode','-MaxWidth','$wire_w','-MaxHeight','$wire_h','-MaxFps','$capture_fps','-BitrateKbps','$bitrate_kbps')
  if ('$window_title' -ne '') { \$argList += @('-Window','$window_title') }
  \$quoted = (\$argList | ForEach-Object { if (\$_ -match '\s') { '\"' + \$_ + '\"' } else { \$_ } }) -join ' '
  \$localDir = Join-Path \$env:LOCALAPPDATA 'couchlink\bin'
  New-Item -ItemType Directory -Force -Path \$localDir | Out-Null
  \$launcher = Join-Path \$localDir 'run-capture.ps1'
  \$utf8 = New-Object System.Text.UTF8Encoding \$false
  [System.IO.File]::WriteAllText(\$launcher, \"powershell.exe -NoProfile -WindowStyle $_ps_style -ExecutionPolicy Bypass \$quoted\", \$utf8)
  if ('$source_mode' -eq 'picker') {
    # Interactive first launch: show the picker on the desktop now.
    Start-Process -WindowStyle $_ps_style powershell.exe -ArgumentList \$argList
  } else {
    schtasks.exe /Delete /TN '$task_name' /F 2>\$null | Out-Null
    \$created = schtasks.exe /Create /TN '$task_name' /SC ONCE /ST 00:00 /RL LIMITED /IT /F \`
      /TR \"powershell.exe -NoProfile -WindowStyle $_ps_style -ExecutionPolicy Bypass -File \$launcher\" 2>&1
    if (\$LASTEXITCODE -ne 0) {
      Write-Host \"schtasks create failed, falling back to Start-Process: \$created\"
      Start-Process -WindowStyle $_ps_style powershell.exe -ArgumentList \$argList
    } else {
      schtasks.exe /Run /TN '$task_name' | Out-Null
    }
  }
" >/dev/null

echo "==> Windows capture launched (source=$source_mode — choose a window in the picker if it appears)"
# Confirm the capture process actually came up. schtasks /Run can succeed while
# the task's PowerShell exits immediately (bad args, missing exe), and the host
# then sits without video — or, before the non-blocking connect fix, blocked
# forever waiting for a socket that never appears.
if command -v tasklist.exe >/dev/null 2>&1; then
  for _ in $(seq 1 25); do
    if tasklist.exe /FI "IMAGENAME eq couchlink-win-capture.exe" 2>/dev/null \
      | grep -qi couchlink-win-capture; then
      exit 0
    fi
    # Window-mode launcher is the PowerShell retry loop until the title exists;
    # that still counts as "capture is starting" for the picker/desktop case we
    # care about here.
    if tasklist.exe /FI "IMAGENAME eq powershell.exe" 2>/dev/null \
      | grep -qi powershell; then
      # Can't tell which powershell — keep waiting briefly for the exe.
      :
    fi
    sleep 0.2
  done
  echo "warning: couchlink-win-capture.exe not visible yet — host will keep trying (picker may still appear)" >&2
fi
exit 0
