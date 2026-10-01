#!/usr/bin/env bash
# Restart the CouchLink online (cloudflared) host stack for a specific game, handling the
# gotchas that make friends see a black "WebCodecs video" page (see docs/RPCS3_FREEZES.md).
#   scripts/restart-stack-for-game.sh mk|bo2|<window-needle> [--hyperv]
# Prints the new join link (also copied to the Windows clipboard). The tunnel hostname changes on
# every restart, so friends need the NEW link.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
case "${1:-}" in
  mk)  NEEDLE=30522;;
  bo2) NEEDLE=31011;;
  ""|-h|--help) sed -n '2,7p' "$0"; exit 2;;
  *)   NEEDLE="$1";;
esac
TRANSPORT=tcp; [ "${2:-}" = "--hyperv" ] && TRANSPORT=hyperv
LOG="/tmp/couchlink-stack-$(date +%Y%m%d-%H%M%S).log"

echo "==> stopping old stack (exact-name matches only; never pkill -f a pattern this shell contains)"
for n in couchlink-signaling couchlink-host turnserver cloudflared; do
  for p in $(pgrep -x "$n" 2>/dev/null); do kill "$p" 2>/dev/null || true; done
done
for p in $(pgrep -f '^bash \./scripts/run\.sh host' 2>/dev/null); do kill "$p" 2>/dev/null || true; done

echo "==> killing Windows capture AND its supervisor (the supervisor respawns it with a stale -Window)"
powershell.exe -NoProfile -Command "Get-CimInstance Win32_Process | ? { \$_.CommandLine -match 'start-win-capture\.ps1' } | % { taskkill /PID \$_.ProcessId /F /T | Out-Null }; Stop-Process -Name couchlink-win-capture -Force -EA SilentlyContinue" 2>&1 | tr -d '\r' | grep -v '^$' || true
sleep 4
rm -f /tmp/couchlink-join-url.txt /tmp/couchlink-win-capture.lock /tmp/couchlink-win-capture.cooling

echo "==> pinning capture window needle '$NEEDLE' in .env.couchlink (it overrides the command-line env)"
[ -f .env.couchlink ] || cp .env.example .env.couchlink
if grep -q '^COUCHLINK_CAPTURE_WINDOW=' .env.couchlink; then
  sed -i "s/^COUCHLINK_CAPTURE_WINDOW=.*/COUCHLINK_CAPTURE_WINDOW=\"$NEEDLE\"/" .env.couchlink
else
  echo "COUCHLINK_CAPTURE_WINDOW=\"$NEEDLE\"" >> .env.couchlink
fi

echo "==> starting host stack (capture transport: $TRANSPORT) -> $LOG"
COUCHLINK_CAPTURE_TRANSPORT="$TRANSPORT" COUCHLINK_CAPTURE_SOURCE=window COUCHLINK_CAPTURE_WINDOW="$NEEDLE" \
  setsid nohup ./scripts/run.sh host --online --force-cloudflare > "$LOG" 2>&1 < /dev/null &
disown
for _ in $(seq 1 40); do sleep 3; [ -s /tmp/couchlink-join-url.txt ] && break; done
[ -s /tmp/couchlink-join-url.txt ] || { echo "no join URL after 2 min; see $LOG" >&2; exit 1; }
tr -d '\r\n' < /tmp/couchlink-join-url.txt | clip.exe

if [ "$TRANSPORT" = tcp ]; then
  echo "==> waiting for capture to attach on :9876"
  ok=0; for _ in $(seq 1 20); do sleep 3; ss -tan 2>/dev/null | grep -q ':9876.*ESTAB' && { ok=1; break; }; done
  if [ "$ok" = 1 ]; then echo "capture ESTABLISHED"; else echo "WARNING: capture not attached; check 'tasklist.exe | grep couchlink-win-capture' and $LOG" >&2; fi
fi
host=$(sed 's#\(https://[^/]*\)/.*#\1#' /tmp/couchlink-join-url.txt)
echo "tunnel: $(curl -s -o /dev/null -w '%{http_code}' "$host/")  (200 = reachable)"
echo; echo "JOIN LINK (on clipboard):"; cat /tmp/couchlink-join-url.txt; echo
