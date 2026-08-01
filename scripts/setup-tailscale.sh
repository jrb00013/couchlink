#!/usr/bin/env bash
# Ensure Tailscale is installed and print bring-up steps for couchlink mesh.
# Does not complete interactive login non-interactively — see docs/MESH.md.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"

usage() {
  cat <<EOF
usage: $0 [--check]

  Install hints + status for Tailscale (PRIME mesh for couchlink --online).
  --check   only print whether a Tailscale IPv4 is available (exit 0/1)

  After \`tailscale up\`, both host and friend on the same tailnet can join via
  http://100.x.y.z:8443/ — ./scripts/run.sh host --online prefers this automatically.
EOF
}

CHECK_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --check) CHECK_ONLY=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; usage >&2; exit 1 ;;
  esac
done

PLATFORM="$(couchlink_detect_platform)"

find_tailscale() {
  if command -v tailscale >/dev/null 2>&1; then
    command -v tailscale
    return 0
  fi
  if command -v tailscale.exe >/dev/null 2>&1; then
    command -v tailscale.exe
    return 0
  fi
  local cand
  for cand in \
    "/mnt/c/Program Files/Tailscale/tailscale.exe" \
    "/mnt/c/Program Files (x86)/Tailscale/tailscale.exe"; do
    if [[ -x "$cand" ]]; then
      echo "$cand"
      return 0
    fi
  done
  return 1
}

if [[ "$CHECK_ONLY" == 1 ]]; then
  if ip="$(couchlink_tailscale_ip 2>/dev/null)"; then
    echo "tailscale ok: $ip"
    exit 0
  fi
  echo "tailscale not ready (not installed, logged out, or no 100.x address)"
  exit 1
fi

echo "==> Tailscale setup for couchlink (PRIME mesh)"

BIN=""
if BIN="$(find_tailscale)"; then
  echo "==> found: $BIN"
else
  echo "==> Tailscale not installed"
  case "$PLATFORM" in
    linux)
      echo "    curl -fsSL https://tailscale.com/install.sh | sh"
      echo "    sudo tailscale up"
      ;;
    wsl)
      echo "    Preferred on WSL: install Tailscale for Windows"
      echo "      https://tailscale.com/download/windows"
      echo "    Or inside WSL: curl -fsSL https://tailscale.com/install.sh | sh"
      ;;
    macos)
      echo "    brew install --cask tailscale   # or Mac App Store"
      echo "    open Tailscale and sign in, or: tailscale up"
      ;;
    *)
      echo "    https://tailscale.com/download"
      ;;
  esac
  echo ""
  echo "Re-run: ./scripts/setup-tailscale.sh --check"
  echo "Then:   ./scripts/run.sh host --online"
  exit 0
fi

if ip="$(couchlink_tailscale_ip 2>/dev/null)"; then
  echo "==> Tailscale up — IPv4 $ip"
  echo "    Friend: install Tailscale, join your tailnet (share this machine if needed),"
  echo "    then open the join URL printed by: ./scripts/run.sh host --online"
  echo "    Native couchlink client preferred for video (WebCodecs wants https)."
  exit 0
fi

echo "==> Tailscale installed but no 100.x address yet — sign in:"
case "$PLATFORM" in
  wsl)
    echo "    Windows: open Tailscale app → Log in"
    echo "    Or WSL:  sudo tailscale up"
    ;;
  *)
    echo "    sudo tailscale up"
    ;;
esac
echo "    Then re-check: ./scripts/setup-tailscale.sh --check"
echo "    Host:          ./scripts/run.sh host --online"
exit 0
