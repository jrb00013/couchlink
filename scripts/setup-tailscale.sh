#!/usr/bin/env bash
# Ensure Tailscale is installed and ready for couchlink paste-link mesh.
# On WSL: installs Tailscale for *Windows* (winget / MSI), not a second Linux daemon.
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

  WSL: installs the Windows Tailscale app (required so 100.x is on the Windows
       NIC that friends / portproxy hit). Native Linux/macOS install the local client.

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

# Install Tailscale for Windows from WSL (elevated once, like enable-upnp).
install_windows_tailscale() {
  local ps_win_exe='C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'
  local ps_win_launch script_w marker run_linux win_user schtasks task_name used_task=0 ec=""

  if [[ -x /mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe ]]; then
    ps_win_launch='/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe'
  elif command -v powershell.exe >/dev/null 2>&1; then
    ps_win_launch="$(command -v powershell.exe)"
  else
    echo "powershell.exe not found — cannot install Windows Tailscale from WSL" >&2
    return 1
  fi

  win_user=""
  if command -v cmd.exe >/dev/null 2>&1; then
    win_user="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r')"
  fi
  win_user="${win_user:-${USER}}"
  run_linux="/mnt/c/Users/${win_user}/AppData/Local/couchlink-run"
  mkdir -p "$run_linux"
  cp -f "$ROOT/scripts/windows/install-tailscale.ps1" "$run_linux/install-tailscale.ps1"
  script_w="$(wslpath -w "$run_linux/install-tailscale.ps1")"
  marker="$run_linux/install-tailscale.exit"
  rm -f "$marker"

  echo "==> installing Tailscale for Windows (UAC once if needed; ${COUCHLINK_TS_UAC_TIMEOUT:-90}s max)…"

  local uac_timeout="${COUCHLINK_TS_UAC_TIMEOUT:-90}"
  schtasks="/mnt/c/Windows/System32/schtasks.exe"
  task_name="CouchlinkInstallTailscale"
  if [[ -x "$schtasks" ]] && "$schtasks" /Query /TN "$task_name" &>/dev/null; then
    if "$schtasks" /Run /TN "$task_name" &>/dev/null; then
      used_task=1
      local waited=0
      while [[ ! -f "$marker" && $waited -lt "$uac_timeout" ]]; do
        sleep 1
        waited=$((waited + 1))
      done
      if [[ -f "$marker" ]]; then
        ec="$(tr -d '\r\n' <"$marker")"
      else
        used_task=0
      fi
    fi
  fi

  if [[ "$used_task" -eq 0 ]]; then
    set +e
    if command -v timeout >/dev/null 2>&1; then
      timeout "$uac_timeout" "$ps_win_launch" -NoProfile -Command \
        "\$p = Start-Process -FilePath '$ps_win_exe' -Verb RunAs -PassThru -Wait -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$script_w'); if (\$null -eq \$p) { exit 1 }; exit \$p.ExitCode"
      ec=$?
      if [[ "$ec" -eq 124 ]]; then
        echo "==> Windows Tailscale UAC wait timed out after ${uac_timeout}s" >&2
      fi
    else
      "$ps_win_launch" -NoProfile -Command \
        "\$p = Start-Process -FilePath '$ps_win_exe' -Verb RunAs -PassThru -Wait -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$script_w'); if (\$null -eq \$p) { exit 1 }; exit \$p.ExitCode"
      ec=$?
    fi
    set -e
    if [[ -f "$marker" ]]; then
      ec="$(tr -d '\r\n' <"$marker")"
    fi
    # UAC denied / no interactive desktop: try non-elevated winget/MSI (often enough).
    if [[ "${ec:-1}" != "0" ]]; then
      echo "==> elevated install failed (exit ${ec:-?}) — retrying without UAC…"
      rm -f "$marker"
      set +e
      if command -v timeout >/dev/null 2>&1; then
        timeout 120 "$ps_win_launch" -NoProfile -ExecutionPolicy Bypass -File "$script_w"
        ec=$?
      else
        "$ps_win_launch" -NoProfile -ExecutionPolicy Bypass -File "$script_w"
        ec=$?
      fi
      set -e
      if [[ -f "$marker" ]]; then
        ec="$(tr -d '\r\n' <"$marker")"
      fi
    fi
  fi

  # Register a no-UAC task for next time (best-effort).
  if [[ -x "$schtasks" ]] && ! "$schtasks" /Query /TN "$task_name" &>/dev/null; then
    "$ps_win_launch" -NoProfile -Command \
      "\$a = New-ScheduledTaskAction -Execute '$ps_win_exe' -Argument '-NoProfile -ExecutionPolicy Bypass -File \"$script_w\"'; \
       \$p = New-ScheduledTaskPrincipal -UserId \$env:USERNAME -RunLevel Highest; \
       Register-ScheduledTask -TaskName '$task_name' -Action \$a -Principal \$p -Force | Out-Null" \
      >/dev/null 2>&1 || true
  fi

  ec="${ec:-1}"
  if [[ "$ec" == "0" ]]; then
    echo "==> Windows Tailscale installer finished OK"
    return 0
  fi
  echo "==> Windows Tailscale install exited $ec" >&2
  return 1
}

try_install_tailscale() {
  case "$PLATFORM" in
    linux)
      if command -v apt-get >/dev/null 2>&1; then
        echo "==> installing Tailscale (official Linux install.sh)…"
        curl -fsSL https://tailscale.com/install.sh | sh \
          || { echo "warning: Tailscale install failed — https://tailscale.com/download/linux" >&2; return 1; }
        return 0
      fi
      ;;
    wsl)
      # Friends/host on WSL must use the *Windows* Tailscale so 100.x is on the
      # Windows NIC (portproxy / friend path). Do not install a second Linux daemon.
      install_windows_tailscale || return 1
      # Refresh discovery after MSI/winget.
      sleep 2
      return 0
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
      # Also kick CLI up (opens browser login when needed).
      if [[ -n "$bin" && -f "$bin" ]]; then
        powershell.exe -NoProfile -Command \
          "Start-Process -FilePath '$(wslpath -w "$bin" 2>/dev/null || echo "$bin")' -ArgumentList 'up'" \
          >/dev/null 2>&1 || true
      fi
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
      echo "    Re-run: ./scripts/setup-tailscale.sh --ensure"
      echo "    Or install manually: https://tailscale.com/download/windows"
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
  echo "==> Tailscale still has no 100.x address — finish sign-in in the Windows app, then:"
  echo "    ./scripts/setup-tailscale.sh --check"
  exit 1
fi

echo "==> Tailscale installed but no 100.x address yet — sign in:"
bring_up_tailscale "$BIN"
echo "    Then: ./scripts/setup-tailscale.sh --check"
echo "    Host: ./scripts/run.sh host --online"
exit 0
