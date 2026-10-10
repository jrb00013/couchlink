#!/usr/bin/env bash
# Pre-flight for a BO2 co-op test night. Read-only: prints PASS/WARN/FAIL per item and the join link.
#   tools/rpcs3-debug/preflight-bo2.sh
set -uo pipefail
R="${RPCS3_DIR:-/mnt/c/Users/josep/RPCS3}"; ok=0; warn=0; fail=0
p() { printf '%-5s %s\n' "$1" "$2"; case "$1" in PASS) ok=$((ok+1));; WARN) warn=$((warn+1));; FAIL) fail=$((fail+1));; esac; }
active=$(tr -d '\r\n' < "$R/versions/ACTIVE.txt"); p PASS "active build: $active"
exe="$R/rpcs3.exe"
t=$(strings -a "$exe" | grep -c 'dump_threads.trigger'); [ "$t" -gt 0 ] && p PASS "guest thread dump trigger present" || p FAIL "exe lacks dump_threads.trigger (older build)"
c=$(strings -a "$exe" | grep -c 'PPU reservation contention'); [ "$c" -gt 0 ] && p PASS "stwcx. contention instrumentation present" || p WARN "no stwcx. histogram in this build (needs commit >= 40500ef)"
[ -f "$R/versions/$active/pdb/rpcs3.pdb" ] && p PASS "PDB staged for $active" || p WARN "no PDB next to $active (dumps cannot be symbolized)"
cfg="$R/config/custom_configs/config_BLUS31011.yml"
g() { tr -d '\r' < "$cfg" | sed -n "s/^  $1: //p" | head -1; }
[ "$(g 'Multithreaded RSX')" = false ] && p PASS "MT RSX off" || p FAIL "MT RSX not off (BO2 menu softlock)"
[ "$(g 'Disable SPU GETLLAR Spin Optimization')" = false ] && p PASS "GETLLAR spin optimization on (default)" || p WARN "GETLLAR spin optimization disabled"
grep -A5 'force SPURS' "$R/config/patch_config.yml" | grep -q 'Enabled: false' && p PASS "unproven SPURS force-complete patch is OFF" || p WARN "SPURS force-complete patch is ON (crashed BO2 once)"
grep -B1 -A5 'hang-detect' "$R/config/patch_config.yml" | grep -q 'Enabled: true' && p PASS "hang-detect NOP patch on" || p WARN "hang-detect patch not enabled"
tl=$(tasklist.exe 2>&1 | tr -d '\r'); if grep -q 'UtilAcceptVsock\|accept4' <<<"$tl"; then p WARN "WSL<->Windows interop failing right now (native watcher still works)"; else grep -qi '^rpcs3.exe' <<<"$tl" && p PASS "RPCS3 running" || p WARN "RPCS3 not running"; fi
n=$(powershell.exe -NoProfile -Command "(Get-CimInstance Win32_Process | ? { \$_.CommandLine -match 'rpcs3-stall-watch\.ps1' } | measure).Count" 2>&1 | tr -d '\r' | tail -1)
[ "${n:-0}" -ge 1 ] 2>/dev/null && p PASS "Windows-native stall watcher running" || p FAIL "native stall watcher NOT running: tools/rpcs3-debug/start-windows-watcher.sh"
free=$(df -BG /mnt/c 2>/dev/null | awk 'NR==2{gsub("G","",$4); print $4}'); [ "${free:-0}" -ge 20 ] && p PASS "disk free ${free} GB" || p WARN "low disk (${free} GB)"
sz=$(du -m "$R/log/RPCS3.log" 2>/dev/null | cut -f1); [ "${sz:-0}" -lt 400 ] && p PASS "RPCS3.log ${sz} MB" || p WARN "RPCS3.log ${sz} MB (verbose; restart soon)"
if [ -s /tmp/couchlink-join-url.txt ]; then h=$(sed 's#\(https://[^/]*\)/.*#\1#' /tmp/couchlink-join-url.txt); code=$(curl -s -o /dev/null -w '%{http_code}' "$h/"); [ "$code" = 200 ] && p PASS "tunnel reachable ($h)" || p FAIL "tunnel not reachable (code $code): scripts/restart-stack-for-game.sh bo2"
  rx() { ss -ti state established '( sport = :9876 )' 2>/dev/null | grep -o 'bytes_received:[0-9]*' | head -1 | cut -d: -f2; }
  a=$(rx); sleep 3; b=$(rx)
  if [ -n "$a" ] && [ -n "$b" ] && [ "$b" -gt "$a" ]; then p PASS "capture flowing: $(( (b-a)/3/1024 )) KB/s into the host"; else p FAIL "capture NOT flowing (friends see no video): scripts/capture-keeper.sh --bg 31011  or restart-stack-for-game.sh bo2"; fi
  pgrep -f 'capture-keeper\.sh' >/dev/null && p PASS "capture keeper running" || p WARN "capture keeper not running: scripts/capture-keeper.sh --bg 31011"
  grep -q 'window 31011\|31011' "$HOME/projects/couchlink/.env.couchlink" && p PASS "capture needle 31011 (BO2)" || p FAIL "capture needle is not 31011"
else p FAIL "no join link: scripts/restart-stack-for-game.sh bo2"; fi
echo; echo "result: $ok pass, $warn warn, $fail fail"
[ -s /tmp/couchlink-join-url.txt ] && { echo; echo "JOIN LINK:"; tr -d '\r\n' < /tmp/couchlink-join-url.txt; echo; }
echo; echo "Test protocol: tools/rpcs3-debug/TEST_NIGHT.md"
