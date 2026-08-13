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

# Kill any stale companion FIRST: a running couchlink-ds-vhid.exe locks its own
# exe on Windows, so `cargo build` below fails with "Access is denied" trying to
# overwrite it — and the old process then never gets replaced, leaving the host
# video-only. (taskkill is idempotent; missing process is fine.)
if command -v taskkill.exe >/dev/null 2>&1; then
  taskkill.exe /IM couchlink-ds-vhid.exe /F >/dev/null 2>&1 || true
fi

echo "==> ensuring DualSense VHID companion is built…"
if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$build_ps1"
else
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$build_ps1" >/dev/null 2>&1
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
backend="${COUCHLINK_DS_VHID_BACKEND:-auto}"
exe_w="$(wslpath -w "$ROOT/target/release/couchlink-ds-vhid.exe")"

# Inbound from the WSL vSwitch is still inbound as far as Windows is concerned.
# Note: no PowerShell backtick continuations here — inside a double-quoted bash
# string a backtick starts command substitution and silently eats the line.
powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "if (-not (Get-NetFirewallRule -DisplayName 'couchlink-ds-vhid-39251' -ErrorAction SilentlyContinue)) { New-NetFirewallRule -DisplayName 'couchlink-ds-vhid-39251' -Direction Inbound -Protocol TCP -LocalPort 39251 -Action Allow -Profile Any -ErrorAction SilentlyContinue | Out-Null }" >/dev/null 2>&1 || true

powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "Start-Process -WindowStyle Minimized -FilePath '$exe_w' -ArgumentList '--bind','$bind','--backend','$backend'" >/dev/null 2>&1 || {
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
