#!/usr/bin/env bash
# One-shot prep so `./scripts/run.sh host --online` needs no separate install/build
# steps: release binaries, browser UI, cloudflared, and (WSL) win-capture.exe.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
export PATH="$(couchlink_tool_path "${HOME:-}")${PATH:+:$PATH}"

MODE="${COUCHLINK_MODE:-local}"
PLATFORM="$(couchlink_detect_platform)"

need_build=0
[[ ! -x "$ROOT/target/release/couchlink-host" ]] && need_build=1
[[ ! -x "$ROOT/target/release/couchlink-signaling" ]] && need_build=1

if [[ "$need_build" == "1" ]]; then
  couchlink_say "==> building couchlink-host + couchlink-signaling (release)…"
  # shellcheck disable=SC1091
  source "$ROOT/scripts/ensure-linux-link-libs.sh"
  cargo build --release -p couchlink-host -p couchlink-signaling
fi

if [[ ! -f "$ROOT/web/dist/index.html" ]]; then
  if command -v npm >/dev/null 2>&1; then
    couchlink_say "==> building player UI (web/dist)…"
    (cd "$ROOT/web" && npm install && npm run build)
  else
    echo "warning: web/dist missing and npm not found — browser friends need: cd web && npm install && npm run build" >&2
  fi
fi

if [[ "$MODE" == "online" && "${COUCHLINK_NO_CLOUDFLARE:-0}" != "1" ]]; then
  # shellcheck disable=SC1091
  source "$ROOT/scripts/lib-online-tunnel.sh"
  if ! couchlink_ensure_cloudflared "$ROOT" >/dev/null 2>&1; then
    echo "warning: cloudflared download failed — HTTPS invite may fall back to IPv6/bore" >&2
  fi
fi

if [[ "$PLATFORM" == "wsl" ]]; then
  case "${COUCHLINK_WINDOWS_CAPTURE:-auto}" in
    0|false|local|off) ;;
    *)
      if [[ "${COUCHLINK_SKIP_WIN_CAPTURE_BUILD:-0}" != "1" ]] \
        && command -v powershell.exe >/dev/null 2>&1; then
        if ! powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass \
          -File "$(wslpath -w "$ROOT/scripts/build-win-capture.ps1")" >/dev/null 2>&1; then
          echo "warning: couchlink-win-capture.exe build failed — install Rust on Windows, then retry" >&2
        fi
      fi
      ;;
  esac
fi

if [[ "$MODE" == "online" ]] && ! command -v turnserver >/dev/null 2>&1; then
  couchlink_vlog "==> coturn not on PATH — start-turn will try to install"
fi
