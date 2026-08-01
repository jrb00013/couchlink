#!/usr/bin/env bash
# Ensure the open-source Tailscale *client binary* exists for Headscale joins.
# NEVER opens login.tailscale.com / Tailscale Inc cloud sign-in.
# NEVER runs Windows MSI/UAC install (that was the bad popup).
#
# Headscale is the control plane; this binary only speaks the protocol with:
#   tailscale up --login-server="$hs" --auth-key="$key"
#
# Prints the binary path on stdout (last line). Logs go to stderr.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"

PLATFORM="$(couchlink_detect_platform)"

if bin="$(couchlink_find_tailscale_bin 2>/dev/null)"; then
  # Prefer a real Linux/macOS binary for Headscale; still accept .exe if that's all we have.
  echo "==> Headscale client binary ready: $bin" >&2
  printf '%s\n' "$bin"
  exit 0
fi

if command -v tailscale >/dev/null 2>&1; then
  bin="$(command -v tailscale)"
  echo "==> Headscale client binary ready: $bin" >&2
  printf '%s\n' "$bin"
  exit 0
fi

echo "==> no mesh client binary — installing Linux client for Headscale (not Tailscale Inc)" >&2
case "$PLATFORM" in
  linux|wsl)
    if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
      curl -fsSL https://tailscale.com/install.sh | sh >&2
    elif command -v sudo >/dev/null 2>&1; then
      curl -fsSL https://tailscale.com/install.sh | sudo sh >&2
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
      echo "install Tailscale.app from https://tailscale.com/download/mac (then use --login-server=Headscale)" >&2
      exit 1
    fi
    ;;
  *)
    echo "unsupported platform for auto client install: $PLATFORM" >&2
    exit 1
    ;;
esac

bin="$(couchlink_find_tailscale_bin 2>/dev/null || true)"
if [[ -z "$bin" ]] && command -v tailscale >/dev/null 2>&1; then
  bin="$(command -v tailscale)"
fi
if [[ -z "$bin" ]]; then
  echo "Tailscale client still missing after install" >&2
  exit 1
fi
echo "==> Headscale client binary ready: $bin" >&2
printf '%s\n' "$bin"
exit 0
