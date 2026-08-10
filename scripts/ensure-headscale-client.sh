#!/usr/bin/env bash
# Ensure the open-source Tailscale *client binary* exists for Headscale joins.
# NEVER opens login.tailscale.com / Tailscale Inc cloud sign-in.
# NEVER runs Windows MSI/UAC install (that was the bad popup).
#
# On WSL: prefer a Linux `tailscale` binary (do not hijack Windows Tailscale.exe /
# Tailscale Inc login). Opt into Windows client with COUCHLINK_HS_ALLOW_WINDOWS_CLIENT=1.
#
# Prints the binary path on stdout. Logs go to stderr.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"

PLATFORM="$(couchlink_detect_platform)"

pick_linux_tailscale() {
  if command -v tailscale >/dev/null 2>&1; then
    local b
    b="$(command -v tailscale)"
    # Reject Windows interop stubs if any
    case "$b" in
      *.exe|/mnt/c/*) return 1 ;;
    esac
    printf '%s' "$b"
    return 0
  fi
  # macOS: the `tailscale` cask may not be on PATH; use the app bundle CLI.
  if [[ "$(uname -s)" == "Darwin" ]] \
    && [[ -x "/Applications/Tailscale.app/Contents/MacOS/Tailscale" ]]; then
    printf '%s\n' "/Applications/Tailscale.app/Contents/MacOS/Tailscale"
    return 0
  fi
  return 1
}

if bin="$(pick_linux_tailscale)"; then
  echo "==> Headscale client binary ready: $bin" >&2
  printf '%s\n' "$bin"
  exit 0
fi

# Optional: allow Windows Tailscale.exe for Headscale (still --login-server only).
if [[ "${COUCHLINK_HS_ALLOW_WINDOWS_CLIENT:-0}" == "1" ]]; then
  if bin="$(couchlink_find_tailscale_bin 2>/dev/null)"; then
    echo "==> Headscale client binary ready (Windows): $bin" >&2
    printf '%s\n' "$bin"
    exit 0
  fi
fi

echo "==> no Linux mesh client — installing for Headscale (not Tailscale Inc, no Windows UAC)" >&2
case "$PLATFORM" in
  linux|wsl)
    if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
      curl -fsSL https://tailscale.com/install.sh | sh >&2
    elif command -v sudo >/dev/null 2>&1; then
      # Non-interactive if possible
      curl -fsSL https://tailscale.com/install.sh | sudo -n sh >&2 \
        || curl -fsSL https://tailscale.com/install.sh | sudo sh >&2
    else
      echo "need root/sudo to install Tailscale client for Headscale" >&2
      exit 1
    fi
    ;;
  macos)
    brew="$(couchlink_brew_bin 2>/dev/null || true)"
    if [[ -n "${brew:-}" ]]; then
      "$brew" install --cask tailscale >&2 || true
    else
      echo "install Tailscale.app, then use --login-server=<Headscale>" >&2
      exit 1
    fi
    ;;
  *)
    echo "unsupported platform for auto client install: $PLATFORM" >&2
    exit 1
    ;;
esac

if bin="$(pick_linux_tailscale)"; then
  echo "==> Headscale client binary ready: $bin" >&2
  printf '%s\n' "$bin"
  exit 0
fi

echo "Headscale client binary not found after install (platform: $PLATFORM)" >&2
exit 1
