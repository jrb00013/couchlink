#!/usr/bin/env bash
# Keeps the Windows capture feeding the host.
#
# Watches TCP :9876 bytes_received. A hung win-capture can flap reconnects
# (accept resets the counter to a new connection's low value) — so we track the
# peak and only count "alive" when bytes STRICTLY increase past the peak. After
# ~30s with no growth, FORCE-kill capture + supervisor and relaunch (tcp).
#
# Host-side fix (bridge.rs silent-reconnect) is the primary heal; this is the
# belt that still works if the host binary is stale.
#   scripts/capture-keeper.sh [--bg|--stop] [window-needle]
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
echo "$(date +%T) keeper: needle=$NEEDLE (peak-bytes watchdog)"
peak=0; dead=0
while true; do
  sleep 10
  cur=$(rx)
  if [ -n "$cur" ] && [ "$cur" -gt "$peak" ] 2>/dev/null; then
    peak=$cur; dead=0; continue
  fi
  dead=$((dead+1))
  # 3 × 10s with no peak growth (~30s). Flapping reconnects that reset the
  # counter to ~0 do NOT clear dead (unlike the old cur!=last heuristic).
  [ "$dead" -lt 3 ] && continue
  echo "$(date +%T) capture dead for ~$((dead*10))s (rx='${cur:-none}' peak=$peak) -> FORCE restart"
  powershell.exe -NoProfile -Command "Get-CimInstance Win32_Process | ? { \$_.CommandLine -match 'start-win-capture\.ps1' } | % { taskkill /PID \$_.ProcessId /F /T | Out-Null }; Get-Process couchlink-win-capture -EA SilentlyContinue | % { taskkill /PID \$_.Id /F /T | Out-Null }" >/dev/null 2>&1
  sleep 4; rm -f /tmp/couchlink-win-capture.lock /tmp/couchlink-win-capture.cooling
  COUCHLINK_WIN_CAPTURE_FORCE=1 COUCHLINK_CAPTURE_TRANSPORT=tcp COUCHLINK_CAPTURE_SOURCE=window COUCHLINK_CAPTURE_WINDOW="$NEEDLE" \
    bash scripts/ensure-win-capture.sh 2>&1 | tail -1 | sed "s/^/$(date +%T) /"
  dead=0; peak=0
done
