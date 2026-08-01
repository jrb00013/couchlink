#!/usr/bin/env bash
# Best-effort local firewall allow for couchlink + Tailscale/Headscale mesh.
# Usage: ./scripts/unblock-firewall.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"

PLATFORM="$(couchlink_detect_platform)"
echo "==> unblock-firewall (platform=$PLATFORM)"

case "$PLATFORM" in
  wsl|windows)
    ps_script="$ROOT/scripts/windows/unblock-firewall.ps1"
    if [[ ! -f "$ps_script" ]]; then
      echo "missing $ps_script" >&2
      exit 1
    fi
    win_user="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r' || true)"
    win_user="${win_user:-$USER}"
    run="/mnt/c/Users/${win_user}/AppData/Local/couchlink-run"
    mkdir -p "$run"
    cp -f "$ps_script" "$run/unblock-firewall.ps1"
    script_w="$(wslpath -w "$run/unblock-firewall.ps1")"
    ps_exe='C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'
    ps_launch='/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe'
    [[ -x "$ps_launch" ]] || ps_launch="$(command -v powershell.exe)"
    echo "==> elevating Windows firewall rules (UAC once)…"
    set +e
    "$ps_launch" -NoProfile -Command \
      "\$p = Start-Process -FilePath '$ps_exe' -Verb RunAs -PassThru -Wait -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$script_w'); if (\$null -eq \$p) { exit 1 }; exit \$p.ExitCode"
    ec=$?
    set -e
    # Non-elevated fallback
    if [[ "$ec" != "0" ]]; then
      echo "==> elevated failed — trying without UAC…"
      "$ps_launch" -NoProfile -ExecutionPolicy Bypass -File "$script_w" || true
    fi
    ;;
  linux)
    echo "==> Linux firewall best-effort (ufw/firewalld/nft)…"
    if command -v ufw >/dev/null 2>&1 && ufw status 2>/dev/null | grep -qi active; then
      sudo ufw allow 41641/udp comment 'tailscale' || true
      sudo ufw allow 3478 comment 'couchlink-turn' || true
      sudo ufw allow 3479/udp comment 'headscale-stun' || true
      sudo ufw allow 8443/tcp comment 'couchlink-signaling' || true
    elif command -v firewall-cmd >/dev/null 2>&1; then
      sudo firewall-cmd --permanent --add-port=41641/udp || true
      sudo firewall-cmd --permanent --add-port=3478/tcp --add-port=3478/udp || true
      sudo firewall-cmd --permanent --add-port=3479/udp || true
      sudo firewall-cmd --permanent --add-port=8443/tcp || true
      sudo firewall-cmd --reload || true
    else
      echo "    no ufw/firewalld — ensure UDP 41641/3478/3479 and TCP 8443 are allowed"
    fi
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
