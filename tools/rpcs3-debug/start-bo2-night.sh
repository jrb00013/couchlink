#!/usr/bin/env bash
# One command to bring everything up for a BO2 test session, in the right order:
#   BO2 (active build) -> Windows-native stall watcher -> CouchLink stack (cloudflared, tcp capture) -> capture keeper -> preflight.
#   tools/rpcs3-debug/start-bo2-night.sh
# Prints the join link. Stop everything with: tools/rpcs3-debug/stop-all.sh
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1
R="${RPCS3_DIR:-/mnt/c/Users/josep/RPCS3}"
echo "==> active build: $(tr -d '\r\n' < "$R/versions/ACTIVE.txt")   PPU priority fix: $(tools/rpcs3-debug/set-prio.sh status | sed 's/.*= //')"
echo "==> launching BO2"
powershell.exe -NoProfile -Command 'Start-Process -FilePath "C:\Users\josep\RPCS3\rpcs3.exe" -WorkingDirectory "C:\Users\josep\RPCS3" -ArgumentList "`"C:\Users\josep\RPCS3\games\Call of Duty - Black Ops II (USA) (En,Fr)\PS3_GAME\USRDIR\EBOOT.BIN`""' >/dev/null 2>&1
echo "==> starting Windows-native stall watcher"
tools/rpcs3-debug/start-windows-watcher.sh 2>&1 | tail -2
echo "==> starting CouchLink stack for BO2 (new join link)"
scripts/restart-stack-for-game.sh bo2 2>&1 | grep -vE '^$' | tail -6
echo "==> starting capture keeper"
scripts/capture-keeper.sh --bg 31011
sleep 20
echo "==> live monitor (run it as a Claude Code Monitor, or in a terminal): python3 -u tools/rpcs3-debug/live-monitor.py"
echo "==> preflight"
tools/rpcs3-debug/preflight-bo2.sh
