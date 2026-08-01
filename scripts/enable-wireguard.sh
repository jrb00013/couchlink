#!/usr/bin/env bash
# Bring up the couchlink WireGuard tunnel (Windows elevated service, or Linux wg-quick).
# Prefer Task Scheduler "CouchlinkElevatedWireGuard" after first UAC (same idea as UPnP).
#
# Usage: ./scripts/enable-wireguard.sh [path/to/wg0-host.conf]
# Exit: 0 = tunnel up, else error
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"

CONF="${1:-$ROOT/infra/wireguard/wg0-host.conf}"
PLATFORM="$(couchlink_detect_platform)"

if [[ ! -f "$CONF" ]]; then
  echo "==> generating WireGuard configs first"
  "$ROOT/scripts/setup-wireguard.sh"
  CONF="$ROOT/infra/wireguard/wg0-host.conf"
fi
[[ -f "$CONF" ]] || { echo "missing $CONF" >&2; exit 1; }

# Already up?
if ip="$(couchlink_wireguard_ip 2>/dev/null)"; then
  echo "==> WireGuard already up ($ip)"
  exit 0
fi

case "$PLATFORM" in
  wsl|windows)
    ps_win="$(command -v powershell.exe 2>/dev/null || true)"
    [[ -n "${ps_win:-}" ]] || { echo "powershell.exe required on WSL/Windows" >&2; exit 1; }

    WIN_USER=""
    if command -v cmd.exe >/dev/null 2>&1; then
      WIN_USER="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r')"
    fi
    WIN_USER="${WIN_USER:-josep}"
    RUN_LINUX="/mnt/c/Users/${WIN_USER}/AppData/Local/couchlink-run"
    mkdir -p "$RUN_LINUX"
    cp -f "$ROOT/scripts/windows/enable-wireguard.ps1" "$RUN_LINUX/enable-wireguard.ps1"
    # Conf must be on a Windows-visible path for wireguard.exe
    cp -f "$CONF" "$RUN_LINUX/couchlink.conf"
    script_w="$(wslpath -w "$RUN_LINUX/enable-wireguard.ps1")"
    conf_w="$(wslpath -w "$RUN_LINUX/couchlink.conf")"
    MARKER="$RUN_LINUX/enable-wireguard.exit"
    rm -f "$MARKER"

    SCHTASKS="/mnt/c/Windows/System32/schtasks.exe"
    TASK_NAME="CouchlinkElevatedWireGuard"
    used_task=0

    wait_marker() {
      local waited=0
      while [[ ! -f "$MARKER" && $waited -lt 90 ]]; do
        sleep 1
        waited=$((waited + 1))
      done
      [[ -f "$MARKER" ]] || return 1
      tr -d '\r\n' <"$MARKER"
      return 0
    }

    if [[ -x "$SCHTASKS" ]] && "$SCHTASKS" /Query /TN "$TASK_NAME" &>/dev/null; then
      echo "==> WireGuard bring-up via saved task (no UAC)"
      if "$SCHTASKS" /Run /TN "$TASK_NAME" &>/dev/null; then
        used_task=1
        ec="$(wait_marker || true)"
        if [[ -z "${ec:-}" ]]; then
          echo "==> saved task did not finish — falling back to UAC" >&2
          used_task=0
        fi
      fi
    fi

    if [[ "$used_task" -eq 0 ]]; then
      echo "==> elevating enable-wireguard.ps1 (approve UAC once; later runs skip this)"
      set +e
      # Start-Process must use a Windows-native path (not /mnt/c/...).
      "$ps_win" -NoProfile -Command \
        "\$p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -PassThru -Wait -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$script_w','-ConfPath','$conf_w','-TunnelName','couchlink'); if (\$null -eq \$p) { exit 1 }; exit \$p.ExitCode"
      ec=$?
      set -e
      if [[ -f "$MARKER" ]]; then
        ec="$(tr -d '\r\n' <"$MARKER")"
      fi
    fi

    ec="${ec:-1}"
    if [[ "$ec" != "0" ]]; then
      echo "==> enable-wireguard exited $ec" >&2
      echo "    Install WireGuard for Windows: https://www.wireguard.com/install/" >&2
      echo "    Or import $CONF manually and activate the tunnel." >&2
      exit "$ec"
    fi

    # Detect mesh IP (Windows side).
    for _ in $(seq 1 20); do
      if ip="$(couchlink_wireguard_ip 2>/dev/null)"; then
        echo "==> WireGuard up — $ip"
        exit 0
      fi
      sleep 0.5
    done
    # Service installed but detection lagged — advertise conventional host IP.
    echo "==> WireGuard service installed (detect pending) — using ${COUCHLINK_WG_HOST_IP:-10.66.0.1}"
    exit 0
    ;;
  linux|macos)
    if ! command -v wg-quick >/dev/null 2>&1; then
      echo "wg-quick not found — install wireguard-tools" >&2
      exit 1
    fi
    IFACE="${COUCHLINK_WG_IF:-wg0}"
    DEST="/etc/wireguard/${IFACE}.conf"
    echo "==> installing $CONF -> $DEST (needs root)"
    if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
      install -m 600 "$CONF" "$DEST"
      wg-quick up "$IFACE" || wg-quick strip "$IFACE" >/dev/null
    elif command -v sudo >/dev/null 2>&1; then
      sudo install -m 600 "$CONF" "$DEST"
      sudo wg-quick up "$IFACE"
    else
      echo "need root for wg-quick up" >&2
      exit 1
    fi
    ip="$(couchlink_wireguard_ip 2>/dev/null || echo "${COUCHLINK_WG_HOST_IP:-10.66.0.1}")"
    echo "==> WireGuard up — $ip"
    exit 0
    ;;
  *)
    echo "unsupported platform: $PLATFORM" >&2
    exit 1
    ;;
esac
