#!/usr/bin/env bash
# Diagnose a "frozen" RPCS3 WITHOUT restarting it. Run from WSL while the game is stuck.
#   tools/rpcs3-debug/rpcs3-hang-diag.sh [--guest] [--dump] [--shot]
# Prints a verdict: LOADING | GUEST_STALL | DEADLOCK | NOT_RUNNING | HEALTHY, plus evidence.
#   --nudge suspend RPCS3 for 2s then resume (HYPOTHESIS: ends cutscene stalls; see docs), then recheck
#   --guest ask RPCS3 (couchlink-play-19984 build+) to log every PPU thread context via dump_threads.trigger
#   --dump  also write a small minidump (stacks+registers) to C:\Users\<you>\rpcs3-hang-<ts>.dmp (pauses RPCS3 a few s)
#   --shot  also save a screenshot of the desktop into the log dir
# Env: RPCS3_DIR (default /mnt/c/Users/josep/RPCS3)  STALL_SECS (default 30)
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
RPCS3_DIR="${RPCS3_DIR:-/mnt/c/Users/josep/RPCS3}"
LOG="$RPCS3_DIR/log/RPCS3.log"
STALL_SECS="${STALL_SECS:-30}"
DUMP=0; SHOT=0; GUEST=0; NUDGE=0
for a in "$@"; do case "$a" in --dump) DUMP=1;; --shot) SHOT=1;; --guest) GUEST=1;; --nudge) NUDGE=1;; *) echo "unknown arg $a" >&2; exit 2;; esac; done
win() { wslpath -w "$1"; }
PS() { powershell.exe -NoProfile -ExecutionPolicy Bypass "$@" 2>&1 | tr -d '\r'; }

# WSL -> Windows .exe interop can time out ("UtilAcceptVsock accept4 failed 110"). Retry, and
# fall back to log freshness so a flaky interop never reads as "RPCS3 is not running".
running=0; interop_ok=0
for try in 1 2 3; do
  tl=$(tasklist.exe 2>&1 | tr -d '\r')
  if grep -q 'UtilAcceptVsock\|accept4 failed' <<<"$tl" || [ -z "$tl" ]; then sleep 2; continue; fi
  interop_ok=1; grep -qi '^rpcs3.exe' <<<"$tl" && running=1; break
done
age=$(( $(date +%s) - $(date -r "$LOG" +%s) ))
if [ "$interop_ok" = 0 ]; then
  echo "WARNING: WSL<->Windows interop is failing (tasklist.exe timed out). Thread/title checks unavailable."
  echo "         Retry in a minute; 'wsl --shutdown' fixes it but KILLS the CouchLink stack running in WSL. Falling back to log freshness (${age}s old)."
  [ "$age" -lt 20 ] && running=1
fi
if [ "$running" = 0 ]; then
  echo "VERDICT: NOT_RUNNING"
  echo "RPCS3 is not running. Last fatal/crash lines:"
  tr -d '\r' < "$LOG" | grep -a -E '^·F |Fatal error|Access violation|Thread terminated|Stack Trace|SIGSEGV' | tail -5 | cut -c1-200
  tr -d '\r' < "$LOG" | grep -a -E 'GUI: Quit|closeEvent' | tail -1 | cut -c1-120
  exit 0
fi
title=$( [ "$interop_ok" = 1 ] && PS -Command '(Get-Process rpcs3 -EA SilentlyContinue).MainWindowTitle' || echo "(unavailable: interop)" )
echo "title : $title"
echo "wall  : $(date +%T)   log last write: $(date -r "$LOG" +%T)   log size: $(du -h "$LOG" | cut -f1)"

echo; echo "== native threads (CPU over 5s) =="
args=(-File "$(win "$HERE/threads.ps1")" -Seconds 5 -Top 12)
if [ "$DUMP" = 1 ]; then args+=(-Dump "C:\\Users\\$USER\\rpcs3-hang-$(date +%Y%m%d-%H%M%S).dmp"); fi
TH=$(PS "${args[@]}"); echo "$TH"
cores=$(echo "$TH" | sed -n 's/^total busy = \([0-9.]*\) cores.*/\1/p')

echo; echo "== guest syscall rate (from the 10s PPU Syscall Usage blocks) =="
tail -c 8000000 "$LOG" | tr -d '\r' > /tmp/rpcs3-diag-tail.txt
read -r usleep_rate blocks <<<"$(python3 - <<'PY'
import re
t=open('/tmp/rpcs3-diag-tail.txt',errors='replace').read().split('\n')
idx=[i for i,l in enumerate(t) if 'PPU Syscall Usage Stats' in l]
def blk(i):
    d={}
    for l in t[i+1:i+60]:
        m=re.match(r'\s*⁂ (\S+) \[(\d+)\]',l)
        if m: d[m.group(1)]=int(m.group(2))
        elif l.startswith('·'): break
    return d
if len(idx)<2: print("0 0")
else:
    a,b=blk(idx[-2]),blk(idx[-1])
    d={k:(b[k]-a.get(k,0))/10 for k in b}
    for k,v in sorted(d.items(),key=lambda x:-x[1])[:6]: print(f"  {k:30s} {v:10.0f}/s",file=__import__('sys').stderr)
    print(int(d.get('sys_timer_usleep',0)), len(idx))
PY
)"
echo "  sys_timer_usleep = ${usleep_rate:-0}/s  (informational only: healthy MK is 17-50k/s, not a stall signal)"

echo; echo "== recent guest activity =="
GAP=$(python3 - <<'PY'
import re
def secs(ts):
    h,m,x=ts.split(':'); return int(h)*3600+int(m)*60+float(x)
t=[l for l in open('/tmp/rpcs3-diag-tail.txt',errors='replace') if re.match(r'^·[A-Z!] \d+:',l)]
noise=re.compile(r'cellAudio|cellMic|sys_event|sys_mmapper|DualSense|Performance|Syscall Usage|PERF:')
last=[l for l in t if not noise.search(l)]
now=re.sub(r'^·[A-Z!] ','',t[-1]).split()[0] if t else None
ev=re.sub(r'^·[A-Z!] ','',last[-1]).split()[0] if last else None
import sys
print("  emu time now        :", now, file=sys.stderr)
print("  last non-audio event:", (last[-1][:150].strip() if last else 'none in window'), file=sys.stderr)
for l in [l for l in t if re.search(r'cellVdec(EndSeq|Close|OpenEx)',l)][-4:]: print("  vdec:", l[:120].strip(), file=sys.stderr)
print(int(secs(now)-secs(ev)) if now and ev else 9999)
PY
)
echo "  game-event silence  : ${GAP}s  (healthy play: a few s; stalled cutscene: 60s+; threshold ${STALL_SECS}s)"
if [ "$GUEST" = 1 ]; then
  echo; echo "== guest thread dump (dump_threads.trigger) =="
  before=$(tr -d '\r' < "$LOG" | grep -a -c 'Guest thread dump requested')
  : > "$RPCS3_DIR/dump_threads.trigger"
  for i in 1 2 3 4 5 6 7 8; do sleep 1; n=$(tr -d '\r' < "$LOG" | grep -a -c 'Guest thread dump requested'); [ "$n" -gt "$before" ] && break; done
  if [ "$n" -gt "$before" ]; then
    out="$RPCS3_DIR/log/guest-threads-$(date +%Y%m%d-%H%M%S).txt"
    tr -d '\r' < "$LOG" | awk '/Guest thread dump requested/{c++} c>=1' | tail -n +1 | awk -v b="$before" 'BEGIN{n=0} /Guest thread dump requested/{n++} n>b' > "$out"
    echo "  dumped. saved: $out ($(wc -l < "$out") lines). Thread headers + PCs:"
    grep -a -E '^--- |^ *(cia|pc|LR|lr)[ =:]|PC:|CIA' "$out" | head -40 | cut -c1-150 | sed 's/^/    /'
  else
    echo "  no dump after 8s: this RPCS3 build lacks the dump trigger (needs couchlink-play-19984 @ 7fd007b or later). Use --dump for a native minidump."
    rm -f "$RPCS3_DIR/dump_threads.trigger"
  fi
fi

if [ "$SHOT" = 1 ]; then s="$(PS -File "$(win "$HERE/screenshot.ps1")" -Out "C:\\Users\\$USER\\rpcs3-hang-shot.png")"; echo; echo "screenshot: $s"; fi

if [ "$NUDGE" = 1 ]; then
  echo; echo "== nudge =="; PS -File "$(win "$HERE/threads.ps1")" -Nudge 2
  echo "  Rerun without --nudge in ~10s: did game-event silence drop and the cutscene advance?"
fi

echo; case "$title" in FPS:*) hasfps=1;; *) hasfps=0;; esac
verdict=HEALTHY
if [ "$hasfps" = 0 ] && [ -n "${cores:-}" ] && awk "BEGIN{exit !(${cores} > 1.5)}"; then verdict=LOADING
elif [ "${GAP:-0}" -ge "$STALL_SECS" ]; then verdict=GUEST_STALL
elif [ -n "${cores:-}" ] && awk "BEGIN{exit !(${cores} < 0.5)}"; then verdict=DEADLOCK
fi
[ -z "${cores:-}" ] && echo "NOTE: no thread data (interop failing); verdict uses log evidence only."
echo "VERDICT: $verdict"
case $verdict in
 LOADING) echo "  Title has no FPS and cores are busy: PPU/shader/SPU compile or overlay. DO NOT KILL. Look at the screen (--shot); wait for it. Killing restarts the work.";;
 GUEST_STALL) echo "  Emulator alive, rendering, but the game has logged nothing for ${GAP}s: a guest wait loop. Recoverable ones end by themselves (MK cutscene: ~2 min). Wait 3-5 min before restarting.
  Capture evidence while it is stuck: rerun with --guest --dump --shot. See docs/RPCS3_FREEZES.md ('Guest stall').";;
 DEADLOCK) echo "  Everything idle and no FPS: a real deadlock. Run with --dump --shot FIRST (evidence dies with the process), then restart.";;
 HEALTHY) echo "  Looks alive. If the picture is frozen, capture/stream is the problem, not RPCS3: see docs/RPCS3_FREEZES.md ('Stream shows black').";;
esac
