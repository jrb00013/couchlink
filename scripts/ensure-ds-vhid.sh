#!/usr/bin/env bash
# From WSL: build (if needed) + launch the DualSense VHID companion on Windows.
#
# The companion is what turns pad frames into a controller that Windows games
# (RPCS3, PCSX2, …) can actually see. Without it the host runs video-only.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

is_wsl() {
  grep -qi microsoft /proc/version 2>/dev/null
}

spec="${COUCHLINK_DS_VHID:-}"
if [[ -z "$spec" ]]; then
  if is_wsl; then
    spec="auto"
  else
    # Native Windows hosts reach the companion over the named pipe; native
    # Linux hosts use uinput directly. Nothing to launch either way.
    exit 0
  fi
fi
case "$spec" in
  0|false|off|skip) exit 0 ;;
esac

if ! is_wsl; then
  exit 0
fi

if ! command -v powershell.exe >/dev/null 2>&1; then
  echo "==> powershell.exe not found — cannot start DualSense companion on Windows" >&2
  exit 1
fi

build_ps1="$(wslpath -w "$ROOT/scripts/build-win-ds-vhid.ps1")"

# Idempotency gate: this script is re-run on every host start AND every time a
# player reports its controller family (emulator_pad.rs). A running companion
# must NOT be killed on those later calls — killing it wipes the per-slot
# SlotRegistry, so the very next reconnect plugs in a brand-new ViGEm target,
# and the emulator (PCSX2/RPCS3) keeps its old binding pointing at the dead
# pad. Player 2 goes "stuck": old pad kept, new pad never bound. Only relaunch
# when the running companion is gone, runs a different backend, or is running
# code older than what's on disk.
backend="${COUCHLINK_DS_VHID_BACKEND:-auto}"
if command -v powershell.exe >/dev/null 2>&1; then
  probe="$(powershell.exe -NoProfile -WindowStyle Hidden -Command "
    \$p = Get-Process couchlink-ds-vhid -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not \$p) { Write-Output 'none'; exit }
    \$cli = (Get-CimInstance Win32_Process -Filter \"ProcessId=\$(\$p.Id)\").CommandLine
    \$bk = 'auto'
    if (\$cli -match '--backend\s+(\S+)') { \$bk = \$Matches[1] }
    \$exe = \$p.Path
    \$exeT = if (\$exe) { (Get-Item \$exe).LastWriteTimeUtc } else { [datetime]::MinValue }
    \$pidStart = \$p.StartTime.ToUniversalTime()
    Write-Output (\"{0}|{1}|{2}\" -f \$bk, \$exeT.Ticks, \$pidStart.Ticks)
  " 2>/dev/null | tr -d '\r' || true)"
  if [[ -n "$probe" && "$probe" != "none" ]]; then
    IFS='|' read -r running_backend exe_ticks start_ticks <<< "$probe"
    if [[ "$running_backend" == "$backend" && -n "$exe_ticks" && "$exe_ticks" -le "$start_ticks" ]]; then
      echo "==> DualSense companion already running (backend=$running_backend) — keeping its pads"
      exit 0
    fi
  fi
fi

# Kill any stale companion FIRST: a running couchlink-ds-vhid.exe locks its own
# exe on Windows, so `cargo build` below fails with "Access is denied" trying to
# overwrite it — and the old process then never gets replaced, leaving the host
# video-only. (taskkill is idempotent; missing process is fine.)
#
# ViGEmBus ties a virtual controller's lifetime to the process that plugged it
# in — there is no vendor API to hand an existing target to a new process, so
# killing the companion here always destroys and recreates the virtual pad,
# even though it lands back on the same nominal XInput slot. An emulator that
# was already open (RPCS3/PCSX2) is holding a handle to the device that just
# got destroyed and, for XInput specifically, most emulators only resolve
# "who's on slot N" at startup or on an explicit rescan, not continuously — so
# it silently stops receiving input until told to look again. This was the
# actual cause of "not registering his pad anymore" during the 2026-08-22
# capture-hang troubleshooting: repeated manual `taskkill` of the companion
# while PCSX2 stayed open the whole time. Surface it instead of staying quiet.
if command -v tasklist.exe >/dev/null 2>&1 \
  && tasklist.exe /FI "IMAGENAME eq couchlink-ds-vhid.exe" 2>/dev/null | grep -qi couchlink-ds-vhid; then
  # Separate calls, not combined /FI: tasklist.exe ANDs multiple /FI filters
  # together rather than ORing them, so one call filtering for both image
  # names at once matches nothing (no process has both names) and this
  # always came back empty.
  # `|| true` on the whole substitution: under `set -eo pipefail`, grep
  # finding no match (the common case — neither emulator open) makes the
  # pipeline's exit status non-zero and would otherwise abort this entire
  # script right here, silently skipping the companion relaunch altogether.
  open_emulators="$( { {
    tasklist.exe /FI "IMAGENAME eq pcsx2-qt.exe" 2>/dev/null
    tasklist.exe /FI "IMAGENAME eq rpcs3.exe" 2>/dev/null
  } | grep -iE 'pcsx2-qt\.exe|rpcs3\.exe' | awk '{print $1}' | sort -u | tr '\n' ' '; } || true )"
  echo "==> restarting DualSense companion — this recreates the virtual pad as a NEW device" >&2
  if [[ -n "${open_emulators// /}" ]]; then
    echo "    already running and will need a controller rescan (or restart) to see it: ${open_emulators}" >&2
  else
    echo "    any emulator that was already open will need a controller rescan (or restart) to see it" >&2
  fi
fi
if command -v taskkill.exe >/dev/null 2>&1; then
  taskkill.exe /IM couchlink-ds-vhid.exe /F >/dev/null 2>&1 || true
fi

echo "==> ensuring DualSense VHID companion is built…"
if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
  powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File "$build_ps1"
else
  powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File "$build_ps1" >/dev/null 2>&1
fi || {
  echo "==> DualSense companion build failed — host will run video-only" >&2
  echo "    detail: COUCHLINK_VERBOSE=1 ./scripts/ensure-ds-vhid.sh" >&2
  exit 0
}

if command -v taskkill.exe >/dev/null 2>&1; then
  taskkill.exe /IM couchlink-ds-vhid.exe /F >/dev/null 2>&1 || true
fi

# Bind all interfaces: the host lives in WSL's own netns, so Windows' loopback
# is not reachable from it — it connects via the default gateway instead.
bind="${COUCHLINK_DS_VHID_BIND:-0.0.0.0}"
exe_w="$(wslpath -w "$ROOT/target/release/couchlink-ds-vhid.exe")"

# Running an unsigned .exe straight off the \\wsl.localhost\... UNC share puts
# it in Windows' "network location" zone, and Explorer/Start-Process throws up
# a blocking "Open File - Security Warning" dialog for that — every single
# launch, with nobody there in a background-spawned process to click it. The
# companion silently never starts; every symptom (no controller, connection
# refused later, a pile of orphaned prompt windows) traces back to this one
# dialog. Copying the binary to a real local NTFS path first means Windows
# never puts it in that zone, so the prompt never fires — this is the actual
# fix, not a workaround around the dialog (registry zone trust, SmartScreen
# policy changes) that would also loosen security for everything else.
exe_local_w="$(powershell.exe -NoProfile -WindowStyle Hidden -Command "
  \$dst = Join-Path \$env:LOCALAPPDATA 'couchlink\bin'
  New-Item -ItemType Directory -Force -Path \$dst | Out-Null
  \$dstExe = Join-Path \$dst 'couchlink-ds-vhid.exe'
  Copy-Item -Path '$exe_w' -Destination \$dstExe -Force
  Write-Output \$dstExe
" 2>/dev/null | tr -d '\r')"
if [[ -z "$exe_local_w" ]]; then
  echo "==> could not stage DualSense companion to a local path — host will run video-only" >&2
  exit 0
fi

# Inbound from the WSL vSwitch is still inbound as far as Windows is concerned.
# Note: no PowerShell backtick continuations here — inside a double-quoted bash
# string a backtick starts command substitution and silently eats the line.
powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -Command "if (-not (Get-NetFirewallRule -DisplayName 'couchlink-ds-vhid-39251' -ErrorAction SilentlyContinue)) { New-NetFirewallRule -DisplayName 'couchlink-ds-vhid-39251' -Direction Inbound -Protocol TCP -LocalPort 39251 -Action Allow -Profile Any -ErrorAction SilentlyContinue | Out-Null }" >/dev/null 2>&1 || true

powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -Command "Start-Process -WindowStyle Hidden -FilePath '$exe_local_w' -ArgumentList '--bind','$bind','--backend','$backend'" >/dev/null 2>&1 || {
  echo "==> could not start DualSense companion — host will run video-only" >&2
  exit 0
}

# Start-Process returns before the companion binds, and the host probes once at
# startup — without this wait it loses the race and falls back to video-only.
# (awk '...exit' closes the pipe early → `ip route` gets SIGPIPE → pipefail +
# set -e kills the script right here, skipping the readiness wait below. The
# `|| true` keeps the gateway probe from becoming a silent 141.)
gw="$(ip route 2>/dev/null | awk '/^default/ {print $3; exit}' || true)"
for _ in $(seq 1 50); do
  for target in 127.0.0.1 ${gw:-}; do
    if timeout 1 bash -c "cat < /dev/null > /dev/tcp/$target/39251" 2>/dev/null; then
      echo "==> DualSense VHID companion ready (TCP $target:39251)"
      exit 0
    fi
  done
  sleep 0.2
done

echo "==> DualSense companion started but not accepting connections yet — host may run video-only" >&2
exit 0
