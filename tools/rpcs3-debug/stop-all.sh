#!/usr/bin/env bash
# Stop RPCS3, the CouchLink stack, capture, keeper and watchers. Run as: bash /tmp/shutdown-all.sh
me=$$
powershell.exe -NoProfile -Command 'Get-Process rpcs3 -EA SilentlyContinue | Stop-Process -Force' >/dev/null 2>&1
for n in couchlink-signaling couchlink-host turnserver cloudflared; do for p in $(pgrep -x "$n" 2>/dev/null); do kill "$p" 2>/dev/null; done; done
for pat in '^bash ./scripts/run.sh host' 'capture-keeper' 'rpcs3-stall-watch'; do
  for p in $(pgrep -f "$pat" 2>/dev/null); do [ "$p" != "$me" ] && [ "$p" != "$PPID" ] && kill "$p" 2>/dev/null; done
done
powershell.exe -NoProfile -Command "Get-CimInstance Win32_Process | ? { \$_.CommandLine -match 'start-win-capture\.ps1|rpcs3-stall-watch\.ps1' } | % { taskkill /PID \$_.ProcessId /F /T | Out-Null }; Get-Process couchlink-win-capture -EA SilentlyContinue | % { taskkill /PID \$_.Id /F /T | Out-Null }" >/dev/null 2>&1
sleep 4
rm -f /tmp/couchlink-join-url.txt /tmp/couchlink-win-capture.lock /tmp/couchlink-win-capture.cooling
echo "--- still running?"
tl=$(tasklist.exe 2>&1 | tr -d '\r'); grep -iE '^(rpcs3|couchlink-win-capture|cloudflared)' <<<"$tl" || echo "windows procs: none"
pgrep -a -x 'couchlink-host|couchlink-signaling|turnserver|cloudflared' || echo "linux stack procs: none"
echo -n "native watcher procs: "; powershell.exe -NoProfile -Command "(Get-CimInstance Win32_Process | ? { \$_.CommandLine -match 'rpcs3-stall-watch\.ps1' } | measure).Count" 2>&1 | tr -d '\r' | tail -1
echo -n "listening ports 8443/9876/3478: "; ss -tln 2>/dev/null | grep -cE ':(8443|9876|3478)\b'
