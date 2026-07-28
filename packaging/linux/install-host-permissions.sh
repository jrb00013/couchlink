#!/usr/bin/env bash
# One-time host permissions (GUI password via pkexec when available).
# Non-technical path: double-click "Couchlink Host Setup" after installing the .deb.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HELPER="$ROOT/native/uinput-helper/couchlink-uinput-helper"

if [[ ! -x "$HELPER" ]]; then
  cc -O2 -Wall -Wextra -o "$HELPER" "$ROOT/native/uinput-helper/couchlink-uinput-helper.c"
fi

if command -v pkexec >/dev/null; then
  echo "==> opening system password prompt (pkexec)…"
  # Pass real user so helper can usermod -aG input
  exec pkexec env SUDO_USER="$USER" DISPLAY="${DISPLAY:-}" XAUTHORITY="${XAUTHORITY:-}" \
    "$HELPER" install-rules
fi

echo "==> pkexec not found — falling back to sudo"
sudo env SUDO_USER="$USER" "$HELPER" install-rules
