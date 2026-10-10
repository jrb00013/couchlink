#!/usr/bin/env bash
# Keeps the Windows capture feeding the host: checks bytes received on :9876 every 15 s; after 3 dead checks
# (no ESTAB, or no new bytes) it kills the capture + its supervisor and relaunches it (tcp transport).
# Needed because the host's own relaunch can leave a hung capture that stays silent for an hour (2026-10-01).
#   scripts/capture-keeper.sh [--bg|--stop] [window-needle]     (needle defaults to COUCHLINK_CAPTURE_WINDOW in .env.couchlink)
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
LOGF=/tmp/capture-keeper.log
case "${1:-}" in
  --stop) for p in $(pgrep -f 'capture-keeper\.sh'); do [ "$p" != "$$" ] && kill "$p" 2>/dev/null; done; echo stopped; exit 0;;
  --bg) shift; for p in $(pgrep -f 'capture-keeper\.sh'); do [ "$p" != "$$" ] && kill "$p" 2>/dev/null; done
        setsid nohup "$0" "$@" > "$LOGF" 2>&1 < /dev/null & disown; echo "started, log $LOGF"; exit 0;;
esac
NEEDLE="${1:-$(sed -n 's/^COUCHLINK_CAPTURE_WINDOW="\?\([^"]*\)"\?/\1/p' .env.couchlink | head -1)}"
rx() { ss -ti state established '( sport = :9876 )' 2>/dev/null | grep -o 'bytes_received:[0-9]*' | head -1 | cut -d: -f2; }
echo "$(date +%T) keeper: needle=$NEEDLE"
last=""; dead=0
while true; do
  sleep 15
  cur=$(rx)
  if [ -n "$cur" ] && [ "$cur" != "$last" ]; then dead=0; last="$cur"; continue; fi
  last="$cur"; dead=$((dead+1))
  [ "$dead" -lt 3 ] && continue
  echo "$(date +%T) capture dead for ~45s (rx='${cur}') -> restarting capture"
  powershell.exe -NoProfile -Command "Get-CimInstance Win32_Process | ? { \$_.CommandLine -match 'start-win-capture\.ps1' } | % { taskkill /PID \$_.ProcessId /F /T | Out-Null }; Get-Process couchlink-win-capture -EA SilentlyContinue | % { taskkill /PID \$_.Id /F /T | Out-Null }" >/dev/null 2>&1
  sleep 4; rm -f /tmp/couchlink-win-capture.lock /tmp/couchlink-win-capture.cooling
  COUCHLINK_CAPTURE_TRANSPORT=tcp COUCHLINK_CAPTURE_SOURCE=window COUCHLINK_CAPTURE_WINDOW="$NEEDLE" bash scripts/ensure-win-capture.sh 2>&1 | tail -1 | sed "s/^/$(date +%T) /"
  dead=0; last=""
done
