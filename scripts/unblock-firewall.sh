#!/usr/bin/env bash
# Best-effort local firewall allow for couchlink + Headscale mesh.
# Usage: ./scripts/unblock-firewall.sh
#
# Dispatches to platform scripts:
#   scripts/windows/unblock-firewall.ps1  (via Helper service when available)
#   scripts/linux/unblock-firewall.sh
#   (macOS handled inline — Application Firewall)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-windows-helper.sh"

PLATFORM="$(couchlink_detect_platform)"
echo "==> unblock-firewall (platform=$PLATFORM)"

unblock_windows() {
  local ps_script="$ROOT/scripts/windows/unblock-firewall.ps1"
  if [[ ! -f "$ps_script" ]]; then
    echo "missing $ps_script" >&2
    return 1
  fi

  if couchlink_helper_ping "$ROOT"; then
    echo "==> Windows firewall via Couchlink Helper (no UAC)"
    couchlink_helper_firewall_unblock "$ROOT"
    return $?
  fi

  if [[ "${COUCHLINK_ALLOW_UAC:-0}" != "1" ]]; then
    couchlink_helper_install_hint
    echo "==> trying non-elevated firewall script (may fail)…"
    local win_user run script_w ps_launch
    win_user="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r' || true)"
    win_user="${win_user:-$USER}"
    run="/mnt/c/Users/${win_user}/AppData/Local/couchlink-run"
    mkdir -p "$run"
    cp -f "$ps_script" "$run/unblock-firewall.ps1"
    script_w="$(wslpath -w "$run/unblock-firewall.ps1")"
    ps_launch='/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe'
    [[ -x "$ps_launch" ]] || ps_launch="$(command -v powershell.exe)"
    "$ps_launch" -NoProfile -ExecutionPolicy Bypass -File "$script_w" || true
    return 0
  fi

  local win_user run script_w ps_exe ps_launch ec
  win_user="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r' || true)"
  win_user="${win_user:-$USER}"
  run="/mnt/c/Users/${win_user}/AppData/Local/couchlink-run"
  mkdir -p "$run"
  cp -f "$ps_script" "$run/unblock-firewall.ps1"
  script_w="$(wslpath -w "$run/unblock-firewall.ps1")"
  ps_exe='C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'
  ps_launch='/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe'
  [[ -x "$ps_launch" ]] || ps_launch="$(command -v powershell.exe)"
  echo "==> elevating Windows firewall rules (COUCHLINK_ALLOW_UAC=1)…"
  set +e
  "$ps_launch" -NoProfile -Command \
    "\$p = Start-Process -FilePath '$ps_exe' -Verb RunAs -PassThru -Wait -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$script_w'); if (\$null -eq \$p) { exit 1 }; exit \$p.ExitCode"
  ec=$?
  set -e
  if [[ "$ec" != "0" ]]; then
    echo "==> elevated failed — trying without UAC…"
    "$ps_launch" -NoProfile -ExecutionPolicy Bypass -File "$script_w" || true
  fi
}

unblock_linux() {
  local sh="$ROOT/scripts/linux/unblock-firewall.sh"
  [[ -x "$sh" || -f "$sh" ]] || {
    echo "missing $sh" >&2
    return 1
  }
  bash "$sh"
}

case "$PLATFORM" in
  windows)
    unblock_windows
    ;;
  wsl)
    # Both sides matter: Windows host NIC + WSL distro firewall.
    unblock_windows || true
    unblock_linux || true
    ;;
  linux)
    unblock_linux
    ;;
  macos)
    echo "==> macOS: granting couchlink/tailscale through Application Firewall (may prompt)…"
    if command -v /usr/libexec/ApplicationFirewall/socketfilterfw >/dev/null 2>&1; then
      for bin in \
        "$(command -v tailscale 2>/dev/null || true)" \
        "$(command -v couchlink-client 2>/dev/null || true)" \
        "$ROOT/target/release/couchlink-client"; do
        [[ -n "$bin" && -x "$bin" ]] || continue
        sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add "$bin" 2>/dev/null || true
        sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp "$bin" 2>/dev/null || true
      done
    fi
    echo "    If joins still fail, System Settings → Network → Firewall → Options"
    ;;
  *)
    echo "unsupported platform: $PLATFORM" >&2
    exit 1
    ;;
esac

echo "OK — firewall unblock attempted"
exit 0
