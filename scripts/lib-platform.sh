#!/usr/bin/env bash
# Shared platform helpers for install.sh / run.sh / start-*.sh
# Source only — do not execute.

# Prints: linux | wsl | macos | windows | unknown
couchlink_detect_platform() {
  case "$(uname -s 2>/dev/null)" in
    Darwin) echo macos ;;
    Linux)
      if [[ -n "${WSL_DISTRO_NAME:-}" ]] \
        || [[ -n "${WSL_INTEROP:-}" ]] \
        || grep -qiE 'microsoft|wsl' /proc/version 2>/dev/null; then
        echo wsl
      else
        echo linux
      fi
      ;;
    MINGW*|MSYS*|CYGWIN*) echo windows ;;
    *) echo unknown ;;
  esac
}

# Resolve a user's home directory (Linux getent + macOS dscl fallback).
couchlink_user_home() {
  local user="${1:-$(id -un)}"
  local home=""
  if command -v getent >/dev/null 2>&1; then
    home="$(getent passwd "$user" 2>/dev/null | cut -d: -f6 || true)"
  fi
  if [[ -z "$home" ]] && [[ "$(uname -s)" == "Darwin" ]]; then
    home="$(dscl . -read "/Users/$user" NFSHomeDirectory 2>/dev/null | awk '{print $2}' || true)"
  fi
  if [[ -z "$home" && -d "/Users/$user" ]]; then
    home="/Users/$user"
  fi
  if [[ -z "$home" && -d "/home/$user" ]]; then
    home="/home/$user"
  fi
  echo "${home:-${HOME:-}}"
}

# Homebrew binary if present (Apple Silicon + Intel paths).
couchlink_brew_bin() {
  if command -v brew >/dev/null 2>&1; then
    command -v brew
    return 0
  fi
  local cand
  for cand in /opt/homebrew/bin/brew /usr/local/bin/brew; do
    if [[ -x "$cand" ]]; then
      echo "$cand"
      return 0
    fi
  done
  return 1
}

# Extra PATH entries for the current platform (Homebrew, cargo, ~/.local/bin).
couchlink_tool_path() {
  local home="${1:-$HOME}"
  local parts=()
  parts+=("$home/.cargo/bin" "$home/.local/bin")
  local brew
  if brew="$(couchlink_brew_bin)"; then
    parts+=("$(dirname "$brew")")
    # Some formulae put keg-only tools under the prefix.
    local prefix
    prefix="$("$brew" --prefix 2>/dev/null || true)"
    if [[ -n "$prefix" ]]; then
      parts+=("$prefix/bin")
    fi
  else
    parts+=(/opt/homebrew/bin /usr/local/bin)
  fi
  parts+=(/usr/bin /bin /usr/sbin /sbin)
  local out="" p
  for p in "${parts[@]}"; do
    [[ -n "$p" ]] || continue
    case ":$out:" in
      *":$p:"*) ;;
      *) out="${out:+$out:}$p" ;;
    esac
  done
  echo "$out"
}

# Portable install of an executable (GNU install -D is Linux-only).
couchlink_install_bin() {
  local src="$1" dest="$2"
  mkdir -p "$(dirname "$dest")"
  if install -m 755 "$src" "$dest" 2>/dev/null; then
    return 0
  fi
  cp "$src" "$dest"
  chmod 755 "$dest"
}

# Best-effort LAN IPv4 for join URLs (Linux + macOS).
couchlink_local_ip() {
  local ip=""
  # Linux (iproute2 / hostname -I)
  if command -v ip >/dev/null 2>&1; then
    ip="$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for (i=1;i<=NF;i++) if ($i=="src") {print $(i+1); exit}}')"
  fi
  if [[ -z "$ip" ]]; then
    ip="$(hostname -I 2>/dev/null | awk '{print $1}')"
  fi
  # macOS
  if [[ -z "$ip" ]]; then
    local iface
    for iface in en0 en1 en2 en3; do
      ip="$(ipconfig getifaddr "$iface" 2>/dev/null || true)"
      [[ -n "$ip" ]] && break
    done
  fi
  if [[ -z "$ip" ]]; then
    ip="$(route -n get default 2>/dev/null | awk '/interface:/{print $2}' | {
      read -r iface
      [[ -n "$iface" ]] && ipconfig getifaddr "$iface" 2>/dev/null
    })"
  fi
  echo "${ip:-}"
}

# WSL networking mode: "mirrored" | "nat" | "" (not WSL / unknown).
#
# Runtime signal wins: only live mirroring gives the WSL instance a global IPv6,
# which is exactly what decides whether a WSL-hosted WireGuard/coturn can answer
# inbound. Falls back to the .wslconfig intent when WSL was not restarted yet.
couchlink_wsl_networking_mode() {
  if [[ "$(couchlink_detect_platform 2>/dev/null)" != "wsl" ]]; then
    return 1
  fi
  if ip -6 addr show scope global 2>/dev/null | grep -q 'inet6 [23]'; then
    echo "mirrored"
    return 0
  fi
  local conf="" win_user=""
  if command -v cmd.exe >/dev/null 2>&1; then
    win_user="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r')"
  fi
  win_user="${win_user:-josep}"
  conf="/mnt/c/Users/${win_user}/.wslconfig"
  if [[ -f "$conf" ]] && grep -qiE '^[[:space:]]*networkingMode[[:space:]]*=[[:space:]]*mirrored' "$conf"; then
    echo "mirrored"
    return 0
  fi
  echo "nat"
  return 0
}

# Quiet-by-default logging for run.sh / start-*.sh (opt in with --verbose).
couchlink_verbose() {
  case "${COUCHLINK_VERBOSE:-0}" in
    1|true|yes|on|TRUE|YES|ON) return 0 ;;
    *) return 1 ;;
  esac
}

# Always-visible status line (join URL hints, fatal paths).
couchlink_say() {
  echo "$@"
}

# Only when COUCHLINK_VERBOSE=1.
couchlink_vlog() {
  if couchlink_verbose; then
    echo "$@"
  fi
}

# Run a command; when quiet, stash stdout/stderr in a log file and print path on failure.
couchlink_run_quiet() {
  local log="$1"
  shift
  if couchlink_verbose; then
    "$@"
    return $?
  fi
  mkdir -p "$(dirname "$log")" 2>/dev/null || true
  if "$@" >"$log" 2>&1; then
    return 0
  fi
  local ec=$?
  echo "==> command failed (exit $ec) — see $log" >&2
  return "$ec"
}
