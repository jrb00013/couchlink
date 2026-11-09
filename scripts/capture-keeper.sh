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
echo "$(date +%T) keeper: needle=$NEEDLE (per-connection growth watchdog)"
# Alive = this connection is receiving real data. A new TCP connection restarts bytes_received near 0, so compare against the
# previous sample (or against 0 after a reconnect), never against an all-time peak: with a peak, every capture restart late in
# a long session looked dead until it re-downloaded the old total (rx=19MB vs peak=305MB, 2026-10-20 04:41) and the keeper
# FORCE-killed a healthy capture, which the viewers saw as another "paused". A hung capture that flaps reconnects only ever
# delivers a few KB per connection, which stays under MIN_GROWTH.
MIN_GROWTH=51200
last=0; dead=0
while true; do
  sleep 10
  cur=$(rx)
  if [ -n "$cur" ]; then
    if [ "$cur" -ge "$last" ] 2>/dev/null; then growth=$((cur-last)); else growth=$cur; fi
    last=$cur
    if [ "$growth" -ge "$MIN_GROWTH" ]; then dead=0; continue; fi
  else
    last=0
  fi
  dead=$((dead+1))
  # 3 x 10s without real data (~30s).
  [ "$dead" -lt 3 ] && continue
  echo "$(date +%T) capture dead for ~$((dead*10))s (rx='${cur:-none}' last=$last) -> FORCE restart"
  powershell.exe -NoProfile -Command "Get-CimInstance Win32_Process | ? { \$_.CommandLine -match 'start-win-capture\.ps1' } | % { taskkill /PID \$_.ProcessId /F /T | Out-Null }; Get-Process couchlink-win-capture -EA SilentlyContinue | % { taskkill /PID \$_.Id /F /T | Out-Null }" >/dev/null 2>&1
  sleep 4; rm -f /tmp/couchlink-win-capture.lock /tmp/couchlink-win-capture.cooling
  COUCHLINK_WIN_CAPTURE_FORCE=1 COUCHLINK_CAPTURE_TRANSPORT=tcp COUCHLINK_CAPTURE_SOURCE=window COUCHLINK_CAPTURE_WINDOW="$NEEDLE" \
    bash scripts/ensure-win-capture.sh 2>&1 | tail -1 | sed "s/^/$(date +%T) /"
  dead=0; last=0
done
