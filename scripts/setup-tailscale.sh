#!/usr/bin/env bash
# Ensure Tailscale is installed and ready for couchlink paste-link mesh.
# Login is interactive unless TS_AUTHKEY is set. See docs/MESH.md.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"

usage() {
  cat <<EOF
usage: $0 [--check|--ensure]

  Tailscale is the easy friend path: same tailnet → paste host join URL.

  --check    print whether a Tailscale IPv4 is available (exit 0/1)
  --ensure   install if missing (when possible) and bring up / sign in

  After both machines are on the same tailnet:
    host:   ./install.sh --host --online     # prints http://100.x… join URL
    friend: ./install.sh --online            # Tailscale already from install; paste URL
EOF
}

MODE="status"
for arg in "$@"; do
  case "$arg" in
    --check) MODE="check" ;;
    --ensure) MODE="ensure" ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; usage >&2; exit 1 ;;
  esac
done

PLATFORM="$(couchlink_detect_platform)"

find_tailscale() {
  couchlink_find_tailscale_bin 2>/dev/null
}

try_install_tailscale() {
  case "$PLATFORM" in
    linux)
      if command -v apt-get >/dev/null 2>&1; then
        echo "==> installing Tailscale (official install.sh)…"
        curl -fsSL https://tailscale.com/install.sh | sh \
          || { echo "warning: Tailscale install failed — https://tailscale.com/download/linux" >&2; return 1; }
        return 0
      fi
      ;;
    wsl)
      echo "==> Preferred on WSL: install Tailscale for Windows"
      echo "      https://tailscale.com/download/windows"
      echo "    Or inside WSL: curl -fsSL https://tailscale.com/install.sh | sh"
      if command -v powershell.exe >/dev/null 2>&1; then
        # Best-effort: open the download page / store.
        powershell.exe -NoProfile -Command "Start-Process 'https://tailscale.com/download/windows'" >/dev/null 2>&1 || true
      fi
      return 1
      ;;
    macos)
      local brew
      brew="$(couchlink_brew_bin 2>/dev/null || true)"
      if [[ -n "${brew:-}" ]]; then
        echo "==> brew install --cask tailscale"
        "$brew" install --cask tailscale \
          || { echo "warning: brew cask failed — https://tailscale.com/download/mac" >&2; return 1; }
        return 0
      fi
      echo "Install Tailscale: https://tailscale.com/download/mac"
      return 1
      ;;
  esac
  echo "Install Tailscale: https://tailscale.com/download"
  return 1
}

bring_up_tailscale() {
  local bin="$1"
  if [[ -n "${TS_AUTHKEY:-}" ]]; then
    echo "==> tailscale up (TS_AUTHKEY)"
    if [[ "$bin" == *.exe ]]; then
      "$bin" up --auth-key="$TS_AUTHKEY" --accept-routes=false 2>/dev/null \
        || "$bin" up --authkey="$TS_AUTHKEY" 2>/dev/null \
        || true
    else
      sudo "$bin" up --auth-key="$TS_AUTHKEY" --accept-routes=false \
        || "$bin" up --auth-key="$TS_AUTHKEY" \
        || true
    fi
    return 0
  fi

  echo "==> Tailscale needs sign-in (same account / shared node as the host)"
  if [[ "$bin" == *.exe ]] || [[ "$PLATFORM" == "wsl" ]]; then
    echo "    Open the Tailscale app on Windows → Log in"
    echo "    Host can share this PC: Tailscale admin → Machines → Share"
    if command -v powershell.exe >/dev/null 2>&1; then
      powershell.exe -NoProfile -Command \
        "Start-Process 'tailscale://'; Start-Process 'https://login.tailscale.com/admin/machines'" \
        >/dev/null 2>&1 || true
    fi
  else
    echo "    Running: sudo $bin up   (browser login)"
    if [[ -t 0 && -t 1 ]]; then
      sudo "$bin" up || "$bin" up || true
    else
      echo "    Re-run in a terminal: sudo $bin up"
    fi
  fi
}

if [[ "$MODE" == "check" ]]; then
  if ip="$(couchlink_tailscale_ip 2>/dev/null)"; then
    echo "tailscale ok: $ip"
    exit 0
  fi
  echo "tailscale not ready (not installed, logged out, or no 100.x address)"
  exit 1
fi

echo "==> Tailscale setup for couchlink (paste-link mesh)"

BIN=""
if ! BIN="$(find_tailscale)"; then
  if [[ "$MODE" == "ensure" ]]; then
    try_install_tailscale || true
    BIN="$(find_tailscale || true)"
  fi
fi

if [[ -z "${BIN:-}" ]]; then
  echo "==> Tailscale not installed"
  case "$PLATFORM" in
    linux)
      echo "    curl -fsSL https://tailscale.com/install.sh | sh && sudo tailscale up"
      ;;
    wsl)
      echo "    Install Windows Tailscale: https://tailscale.com/download/windows"
      ;;
    macos)
      echo "    brew install --cask tailscale   # or Mac App Store, then sign in"
      ;;
    *)
      echo "    https://tailscale.com/download"
      ;;
  esac
  echo ""
  echo "Then: ./scripts/setup-tailscale.sh --check"
  [[ "$MODE" == "ensure" ]] && exit 1
  exit 0
fi

echo "==> found: $BIN"

if ip="$(couchlink_tailscale_ip 2>/dev/null)"; then
  echo "==> Tailscale up — IPv4 $ip"
  echo "    Friend flow: ./install.sh --online  (paste host join URL)"
  echo "    Paste the host join URL (http://${ip}:8443/?… or whatever host printed)."
  exit 0
fi

if [[ "$MODE" == "ensure" ]]; then
  bring_up_tailscale "$BIN"
  sleep 2
  if ip="$(couchlink_tailscale_ip 2>/dev/null)"; then
    echo "==> Tailscale up — IPv4 $ip"
    exit 0
  fi
  echo "==> Tailscale still has no 100.x address — finish sign-in, then re-check"
  echo "    ./scripts/setup-tailscale.sh --check"
  exit 1
fi

echo "==> Tailscale installed but no 100.x address yet — sign in:"
bring_up_tailscale "$BIN"
echo "    Then: ./scripts/setup-tailscale.sh --check"
echo "    Host: ./scripts/run.sh host --online"
exit 0
