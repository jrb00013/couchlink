#!/usr/bin/env bash
# Copy the current Couchlink friend join URL to the clipboard.
#
#   ./scripts/copy-join-url.sh
#   HOST_LOG=/tmp/couchlink-stack-v22.log ./scripts/copy-join-url.sh
#   JOIN_URL='https://…' ./scripts/copy-join-url.sh
set -euo pipefail

find_join_url() {
  local log="$1"
  rg -o 'https://[^[:space:]]+trycloudflare\.com/\?s=[^[:space:]]+' "$log" 2>/dev/null | tail -1 \
    || rg -o 'https?://[^[:space:]]+8443/\?s=[^[:space:]]+' "$log" 2>/dev/null | tail -1 \
    || true
}

pick_stack_log() {
  local f
  if [[ -n "${HOST_LOG:-}" && -f "$HOST_LOG" ]]; then
    printf '%s\n' "$HOST_LOG"
    return
  fi
  for f in /tmp/couchlink-stack-v{30..1}.log /tmp/couchlink-stack.log; do
    [[ -f "$f" ]] || continue
    if [[ -n "$(find_join_url "$f")" ]]; then
      printf '%s\n' "$f"
      return
    fi
  done
  for f in /tmp/couchlink-stack-v{30..1}.log /tmp/couchlink-stack.log; do
    [[ -f "$f" ]] && printf '%s\n' "$f" && return
  done
}

copy_to_clipboard() {
  local text="$1"
  if command -v clip.exe >/dev/null 2>&1; then
    printf '%s' "$text" | clip.exe
    return
  fi
  if command -v wl-copy >/dev/null 2>&1; then
    printf '%s' "$text" | wl-copy
    return
  fi
  if command -v xclip >/dev/null 2>&1; then
    printf '%s' "$text" | xclip -selection clipboard
    return
  fi
  if command -v xsel >/dev/null 2>&1; then
    printf '%s' "$text" | xsel --clipboard --input
    return
  fi
  echo "error: no clipboard tool (clip.exe, wl-copy, xclip, xsel)" >&2
  exit 1
}

JOIN_URL="${JOIN_URL:-}"
if [[ -z "$JOIN_URL" ]]; then
  log="$(pick_stack_log || true)"
  if [[ -n "${log:-}" ]]; then
    JOIN_URL="$(find_join_url "$log")"
  fi
fi

if [[ -z "$JOIN_URL" ]]; then
  echo "error: no join URL — start the stack or set JOIN_URL / HOST_LOG" >&2
  exit 1
fi

copy_to_clipboard "$JOIN_URL"
echo "$JOIN_URL"
echo "==> copied to clipboard"
