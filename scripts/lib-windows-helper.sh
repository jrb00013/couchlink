#!/usr/bin/env bash
# Prefer Couchlink Windows Helper service (no UAC) for privileged Windows prep.
# Preference: helper pipe → legacy Scheduled Task → COUCHLINK_ALLOW_UAC=1 RunAs.
#
# Usage: source this file from enable-upnp.sh / unblock-firewall.sh

# shellcheck disable=SC2034

couchlink_helper_win_user() {
  local u=""
  if command -v cmd.exe >/dev/null 2>&1; then
    u="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r')"
  fi
  echo "${u:-josep}"
}

couchlink_helper_ps_launch() {
  if [[ -x /mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe ]]; then
    echo /mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe
  else
    command -v powershell.exe 2>/dev/null || true
  fi
}

couchlink_helper_script_w() {
  local root="${1:?}"
  local win_user run_linux
  win_user="$(couchlink_helper_win_user)"
  run_linux="/mnt/c/Users/${win_user}/AppData/Local/couchlink-run"
  mkdir -p "$run_linux"
  cp -f "$root/scripts/windows/call-helper.ps1" "$run_linux/call-helper.ps1"
  wslpath -w "$run_linux/call-helper.ps1"
}

# Returns 0 if helper answers ping.
couchlink_helper_ping() {
  local root="${1:?}"
  local ps script_w
  ps="$(couchlink_helper_ps_launch)"
  [[ -n "$ps" ]] || return 1
  script_w="$(couchlink_helper_script_w "$root")"
  "$ps" -NoProfile -ExecutionPolicy Bypass -File "$script_w" -Op ping >/dev/null 2>&1
}

# online_prep via helper. Prints status line. Exit: 0, 2, or other.
# If the installed enable-upnp.ps1 still has #Requires -RunAsAdministrator
# (rejects LocalSystem), fall back to firewall_unblock + portproxy probe so
# --online still works without another UAC.
couchlink_helper_online_prep() {
  local root="${1:?}"
  shift
  local skip_map=0 wsl_ip=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --skip-map) skip_map=1 ;;
      --wsl-ip) wsl_ip="${2:-}"; shift ;;
    esac
    shift || true
  done
  local ps script_w args ec
  ps="$(couchlink_helper_ps_launch)"
  [[ -n "$ps" ]] || return 1
  script_w="$(couchlink_helper_script_w "$root")"
  args=(-NoProfile -ExecutionPolicy Bypass -File "$script_w" -Op online_prep)
  if [[ "$skip_map" == "1" ]]; then
    args+=(-SkipMap)
  fi
  if [[ -n "$wsl_ip" ]]; then
    args+=(-WslIp "$wsl_ip")
  fi
  set +e
  "$ps" "${args[@]}"
  ec=$?
  set -e
  if [[ "$ec" -eq 0 || "$ec" -eq 2 ]]; then
    return "$ec"
  fi

  echo "==> helper online_prep exit $ec — fallback: firewall via helper + portproxy check" >&2
  set +e
  couchlink_helper_firewall_unblock "$root" >/dev/null
  set -e
  if couchlink_helper_portproxy_ok; then
    if [[ "$skip_map" == "1" ]]; then
      return 0
    fi
    # Windows prep OK; UPnP map unknown / skipped by fallback
    return 2
  fi
  return "$ec"
}

# True if Windows already has couchlink portproxy for 8443 (and ideally 3478).
couchlink_helper_portproxy_ok() {
  local out
  out="$(netsh.exe interface portproxy show all 2>/dev/null | tr -d '\r' || true)"
  [[ "$out" == *8443* ]] || return 1
  return 0
}

couchlink_helper_firewall_unblock() {
  local root="${1:?}"
  local ps script_w ec
  ps="$(couchlink_helper_ps_launch)"
  [[ -n "$ps" ]] || return 1
  script_w="$(couchlink_helper_script_w "$root")"
  set +e
  "$ps" -NoProfile -ExecutionPolicy Bypass -File "$script_w" -Op firewall_unblock
  ec=$?
  set -e
  return "$ec"
}

couchlink_helper_install_hint() {
  cat >&2 <<'EOF'
==> Couchlink Helper service not available (needed for Windows firewall/portproxy without UAC).
    Install once: ./scripts/install-windows-helper.sh  (UAC once)
    Or: packaging/windows/build-helper-installer.ps1 → CouchlinkHelper-Setup.exe
    Dev escape hatch: COUCHLINK_ALLOW_UAC=1 (old interactive elevation)
EOF
}
