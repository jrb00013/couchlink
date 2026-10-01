#!/usr/bin/env bash
# Background watcher: detects an RPCS3 guest stall by GAME-EVENT SILENCE and captures evidence
# automatically, so nobody has to be at the keyboard (evidence dies with the process / the stall).
#   tools/rpcs3-debug/rpcs3-stall-watch.sh [--nudge]        (run once; it daemonizes if --bg)
#   tools/rpcs3-debug/rpcs3-stall-watch.sh --bg                start detached, log to /tmp/rpcs3-stall-watch.log
# Per episode (silence >= STALL_SECS, default 30) writes ~/rpcs3-stalls/<timestamp>/ with:
#   diag.txt (rpcs3-hang-diag.sh --shot --dump), log-slice.txt, threads-*.txt, result.txt (duration, nudge effect)
# After NUDGE_AT (default 60) s of silence it suspends RPCS3 for 2 s then resumes (hypothesis test, see
# docs/RPCS3_FREEZES.md) only with --nudge (off by default: disproven), and records whether the stall ended within 15 s.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
RPCS3_DIR="${RPCS3_DIR:-/mnt/c/Users/josep/RPCS3}"; LOG="$RPCS3_DIR/log/RPCS3.log"
STALL_SECS="${STALL_SECS:-30}"; NUDGE_AT="${NUDGE_AT:-60}"; OUT_ROOT="${STALL_OUT:-$HOME/rpcs3-stalls}"
NUDGE=0; [ "${1:-}" = "--nudge" ] && NUDGE=1   # nudge disproven 2026-10-01 (stall ended 323s after it); off by default
if [ "${1:-}" = "--bg" ]; then
  pgrep -f 'rpcs3-stall-watch\.sh$' | grep -qv "^$$\$" && { echo "already running"; exit 0; }
  setsid nohup "$0" > /tmp/rpcs3-stall-watch.log 2>&1 < /dev/null & disown; echo "started (log /tmp/rpcs3-stall-watch.log, evidence in $OUT_ROOT)"; exit 0
fi
mkdir -p "$OUT_ROOT"
silence() { # seconds between newest log line and newest non-audio game event; 0 if unknown
  tail -c 3000000 "$LOG" 2>/dev/null | tr -d '\r' | python3 -c '
import re,sys
def secs(ts):
    h,m,x=ts.split(":"); return int(h)*3600+int(m)*60+float(x)
noise=re.compile(r"cellAudio|cellMic|sys_event|sys_mmapper|DualSense|Performance|Syscall Usage|PERF:")
t=[l for l in sys.stdin if re.match(r"^·[A-Z!] \d+:",l)]
g=[l for l in t if not noise.search(l)]
if not t or not g: print(0)
else:
    f=lambda l: re.sub(r"^·[A-Z!] ","",l).split()[0]
    print(int(secs(f(t[-1]))-secs(f(g[-1]))))'
}
episode=""; nudged=0; started=0; nudge_t=0
echo "$(date +%T) watching $LOG (stall >= ${STALL_SECS}s, nudge=${NUDGE} at ${NUDGE_AT}s)"
while true; do
  sleep 5
  [ -f "$LOG" ] || continue
  s=$(silence); s=${s:-0}
  if [ -z "$episode" ] && [ "$s" -ge "$STALL_SECS" ]; then
    episode="$OUT_ROOT/$(date +%Y%m%d-%H%M%S)"; mkdir -p "$episode"; started=$(date +%s); nudged=0
    echo "$(date +%T) STALL detected (silence ${s}s) -> $episode"
    tail -n 600 "$LOG" | tr -d '\r' | grep -a -vE 'DualSense|Performance|PERF:' | cut -c1-260 > "$episode/log-slice.txt"
    "$HERE/rpcs3-hang-diag.sh" --shot --dump > "$episode/diag.txt" 2>&1
    cp /mnt/c/Users/"$USER"/rpcs3-hang-shot.png "$episode/" 2>/dev/null
    for f in /mnt/c/Users/"$USER"/rpcs3-hang-*.dmp*; do [ -f "$f" ] && mv "$f" "$episode/" 2>/dev/null; done
    d=$(ls "$episode"/*.dmp 2>/dev/null | head -1); if [ -n "$d" ] && [ -x "$HOME/.venvs/md/bin/python" ]; then "$HOME/.venvs/md/bin/python" "$HERE/analyze-dump.py" "$d" > "$episode/dump-threads.txt" 2>&1; fi
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w "$HERE/threads.ps1")" -Seconds 5 -Top 15 2>&1 | tr -d '\r' > "$episode/threads-1.txt"
    echo "$(date +%T) evidence captured"
  elif [ -n "$episode" ]; then
    el=$(( $(date +%s) - started ))
    if [ "$s" -lt 10 ]; then
      { echo "stall lasted ~${el}s wall (incl. ~15s capture); nudged=${nudged}"; [ "$nudged" = 1 ] && echo "nudge at +${nudge_t}s; ended $(( el - nudge_t ))s after nudge"; } > "$episode/result.txt"
      echo "$(date +%T) stall ended: $(tr '\n' ' ' < "$episode/result.txt")"
      episode=""; nudged=0
    elif [ "$NUDGE" = 1 ] && [ "$nudged" = 0 ] && [ "$s" -ge "$NUDGE_AT" ]; then
      nudged=1; nudge_t=$el
      powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(wslpath -w "$HERE/threads.ps1")" -Nudge 2 2>&1 | tr -d '\r' > "$episode/nudge.txt"
      echo "$(date +%T) nudged (silence ${s}s)"
    fi
  fi
done
