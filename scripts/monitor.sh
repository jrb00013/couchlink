#!/usr/bin/env bash
# Live dashboard for a running couchlink host session.
#
# Usage:
#   ./scripts/run.sh host --online --force-cloudflare 2>&1 | tee /tmp/couchlink-host.log
#   ./scripts/monitor.sh                      # tails /tmp/couchlink-host.log
#   ./scripts/monitor.sh /path/to/host.log    # tails a specific log file
#   COUCHLINK_LOG=/path/to/host.log ./scripts/monitor.sh
#
# Parses the same verbose `couchlink-host --verbose` log lines this session's
# troubleshooting was done by hand against (player joins/leaves, per-slot
# RESULT {...} pad-link outcomes, capture staleness, TURN/ICE warnings, SCTP
# corruption) and renders a redrawing terminal dashboard instead of grepping
# the log by hand every time something looks wrong. Read-only: never touches
# the running host, never edits any config — a diagnostic tool only.
set -uo pipefail

LOG="${1:-${COUCHLINK_LOG:-/tmp/couchlink-host.log}}"
if [[ ! -f "$LOG" ]]; then
  echo "error: log file not found: $LOG" >&2
  echo "  start the host redirecting/teeing output there first, e.g.:" >&2
  echo "  ./scripts/run.sh host --online --force-cloudflare 2>&1 | tee $LOG" >&2
  exit 1
fi

BOLD=""; DIM=""; RED=""; GREEN=""; YELLOW=""; CYAN=""; RESET=""
if [[ -t 1 ]] && command -v tput >/dev/null 2>&1 && [[ "$(tput colors 2>/dev/null || echo 0)" -ge 8 ]]; then
  BOLD="$(tput bold)"; DIM="$(tput dim)"; RED="$(tput setaf 1)"; GREEN="$(tput setaf 2)"
  YELLOW="$(tput setaf 3)"; CYAN="$(tput setaf 6)"; RESET="$(tput sgr0)"
fi

# Per-slot state, keyed by couchlink slot number (1-based; game player = slot+1).
declare -A SLOT_STATE      # connected | left
declare -A SLOT_BACKEND
declare -A SLOT_HANDLER
declare -A SLOT_DEVICE
declare -A SLOT_RPCS3
declare -A SLOT_PCSX2
declare -A SLOT_EPOCH

STREAM_LAST_TS=""
STREAM_FPS=""
STREAM_FRAMES=""
CAPTURE_STATE="unknown"
TURN_FAILURES=0
SCTP_ERRORS=0
FRAME_BUDGET_DROPS=0
WARN_TOTAL=0
LAST_EVENT=""
START_TS="$(date +%s)"
PLAYERS_OCCUPIED=0
PLAYERS_MAX=0

redraw() {
  local now elapsed
  now="$(date +%s)"
  elapsed=$((now - START_TS))
  clear
  echo "${BOLD}${CYAN}== couchlink live monitor ==${RESET}  ${DIM}(${LOG}, watching ${elapsed}s)${RESET}"
  echo

  echo "${BOLD}Players: ${PLAYERS_OCCUPIED}/${PLAYERS_MAX}${RESET}"
  local any=0
  for slot in $(printf '%s\n' "${!SLOT_STATE[@]}" | sort -n); do
    any=1
    local state="${SLOT_STATE[$slot]}"
    local badge="${GREEN}●${RESET}"
    [[ "$state" == "left" ]] && badge="${DIM}○${RESET}"
    local player=$((slot + 1))
    local backend="${SLOT_BACKEND[$slot]:-?}"
    local handler="${SLOT_HANDLER[$slot]:-?}"
    local device="${SLOT_DEVICE[$slot]:-?}"
    local rpcs3="${SLOT_RPCS3[$slot]:-?}"
    local pcsx2="${SLOT_PCSX2[$slot]:-?}"
    printf "  %s slot %s -> P%s  backend=%-8s handler=%-8s device=%-16s" \
      "$badge" "$slot" "$player" "$backend" "$handler" "$device"
    color_status() {
      case "$1" in
        already|linked) echo "${GREEN}$1${RESET}" ;;
        skipped) echo "${YELLOW}$1${RESET}" ;;
        failed) echo "${RED}$1${RESET}" ;;
        *) echo "${DIM}$1${RESET}" ;;
      esac
    }
    printf "  rpcs3=%s pcsx2=%s\n" "$(color_status "$rpcs3")" "$(color_status "$pcsx2")"
  done
  [[ "$any" == "0" ]] && echo "  ${DIM}(nobody connected yet)${RESET}"
  echo

  echo "${BOLD}Capture:${RESET}"
  local cap_color="$DIM"
  case "$CAPTURE_STATE" in
    healthy) cap_color="$GREEN" ;;
    stale|lost) cap_color="$RED" ;;
    relaunching) cap_color="$YELLOW" ;;
  esac
  printf "  state=%s%s%s" "$cap_color" "$CAPTURE_STATE" "$RESET"
  if [[ -n "$STREAM_FPS" ]]; then
    printf "  last: %s fps, %s frames total" "$STREAM_FPS" "$STREAM_FRAMES"
  fi
  echo
  echo

  echo "${BOLD}Warnings seen:${RESET}"
  local turn_color="$GREEN"; [[ "$TURN_FAILURES" -gt 5 ]] && turn_color="$YELLOW"
  local sctp_color="$GREEN"; [[ "$SCTP_ERRORS" -gt 20 ]] && sctp_color="$RED"
  printf "  turn-alloc-failures=%s%s%s  sctp-malformed=%s%s%s  frame-budget-drops=%s\n" \
    "$turn_color" "$TURN_FAILURES" "$RESET" "$sctp_color" "$SCTP_ERRORS" "$RESET" "$FRAME_BUDGET_DROPS"
  if [[ "$SCTP_ERRORS" -gt 20 ]]; then
    echo "  ${RED}⚠ sustained SCTP corruption on some player's data channel — see docs/INCIDENT-*.md${RESET}"
  fi
  echo

  if [[ -n "$LAST_EVENT" ]]; then
    echo "${BOLD}Last event:${RESET} ${DIM}${LAST_EVENT}${RESET}"
  fi
  echo
  echo "${DIM}Ctrl-C to stop. Read-only — this never touches the running host.${RESET}"
}

parse_result_field() {
  # parse_result_field '{"player":2,"backend":"xbox360",...}' player
  local json="$1" field="$2"
  echo "$json" | grep -oE "\"${field}\"[[:space:]]*:[[:space:]]*\"?[^,\"}]*\"?" \
    | sed -E "s/\"${field}\"[[:space:]]*:[[:space:]]*//; s/^\"//; s/\"\$//"
}

process_line() {
  local line="$1"
  case "$line" in
    *"player joined session"*"slot"*)
      local slot
      slot="$(echo "$line" | grep -oE 'slot [0-9]+' | grep -oE '[0-9]+')"
      [[ -n "$slot" ]] && { SLOT_STATE[$slot]="connected"; LAST_EVENT="player joined slot $slot"; }
      ;;
    *"player left (slot"*)
      local slot
      slot="$(echo "$line" | grep -oE 'slot [0-9]+' | grep -oE '[0-9]+')"
      [[ -n "$slot" ]] && { SLOT_STATE[$slot]="left"; LAST_EVENT="player left slot $slot"; }
      ;;
    *"players: "*"/"*)
      local frac
      frac="$(echo "$line" | grep -oE '[0-9]+/[0-9]+')"
      PLAYERS_OCCUPIED="${frac%%/*}"
      PLAYERS_MAX="${frac##*/}"
      ;;
    *"RESULT {"*)
      local json player slot
      json="${line#*RESULT }"
      player="$(parse_result_field "$json" player)"
      [[ -n "$player" ]] || return
      slot=$((player - 1))
      SLOT_BACKEND[$slot]="$(parse_result_field "$json" backend)"
      SLOT_HANDLER[$slot]="$(parse_result_field "$json" handler)"
      SLOT_DEVICE[$slot]="$(parse_result_field "$json" device)"
      SLOT_RPCS3[$slot]="$(parse_result_field "$json" rpcs3)"
      SLOT_PCSX2[$slot]="$(parse_result_field "$json" pcsx2)"
      LAST_EVENT="pad reconciled for P${player}: rpcs3=${SLOT_RPCS3[$slot]} pcsx2=${SLOT_PCSX2[$slot]}"
      ;;
    *"streaming"*"fps"*"frames total"*)
      STREAM_FPS="$(echo "$line" | grep -oE '[0-9.]+ fps' | head -1 | grep -oE '[0-9.]+')"
      STREAM_FRAMES="$(echo "$line" | grep -oE '[0-9]+ frames total' | grep -oE '[0-9]+')"
      CAPTURE_STATE="healthy"
      ;;
    *"Hyper-V capture link lost"*|*"Windows capture client lost"*|*"no frame from win-capture"*)
      CAPTURE_STATE="lost"
      LAST_EVENT="capture link lost/stale"
      ;;
    *"relaunching it"*)
      CAPTURE_STATE="relaunching"
      LAST_EVENT="win-capture auto-relaunch triggered"
      ;;
    *"Hyper-V capture socket connected"*|*"Hyper-V capture socket reconnected"*|*"Windows capture client connected"*)
      CAPTURE_STATE="healthy"
      ;;
    *"Failed to allocate on turn.Client"*)
      TURN_FAILURES=$((TURN_FAILURES + 1))
      ;;
    *"unable to parse SCTP packet chunk too short"*)
      SCTP_ERRORS=$((SCTP_ERRORS + 1))
      ;;
    *"frame push exceeded budget"*)
      FRAME_BUDGET_DROPS=$((FRAME_BUDGET_DROPS + 1))
      ;;
  esac
  [[ "$line" == *"WARN"* ]] && WARN_TOTAL=$((WARN_TOTAL + 1))
}

trap 'echo; echo "monitor stopped"; exit 0' INT TERM

# Prime state from whatever is already in the log, then follow new lines.
while IFS= read -r line; do
  process_line "$line"
done < "$LOG"
redraw

LAST_REDRAW="$(date +%s)"
tail -n0 -F "$LOG" 2>/dev/null | while IFS= read -r line; do
  process_line "$line"
  now="$(date +%s)"
  if (( now - LAST_REDRAW >= 1 )); then
    redraw
    LAST_REDRAW="$now"
  fi
done
