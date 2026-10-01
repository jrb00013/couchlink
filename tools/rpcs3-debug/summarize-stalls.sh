#!/usr/bin/env bash
# Summarize stall episodes captured by the Windows-native watcher (and the WSL one).
#   tools/rpcs3-debug/summarize-stalls.sh [since-HHMM]
set -uo pipefail
since="${1:-0000}"
for root in "/mnt/c/Users/$USER/rpcs3-stalls" "$HOME/rpcs3-stalls"; do
  [ -d "$root" ] || continue
  echo "== $root"
  for d in "$root"/2*; do
    [ -d "$d" ] || continue; b=$(basename "$d"); hm=${b:9:4}; [ "$hm" \< "$since" ] && continue
    r=$(tr '\n' ' ' < "$d/result.txt" 2>/dev/null); [ -z "$r" ] && r="(still running or watcher stopped)"
    gd=$(ls "$d"/guest-threads*.txt 2>/dev/null | head -1)
    hot=""; [ -n "$gd" ] && hot=$(grep -a -A3 'stwcx./stdcx. failures since last report' "$gd" 2>/dev/null | head -4 | tr '\n' ' ' | cut -c1-110)
    echo "$b  $r ${hot:+| stcx: $hot}"
  done
done
