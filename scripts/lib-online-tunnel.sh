# Sourced by run.sh — outbound reachability when router UPnP is unavailable.
# Prefer: HTTPS (cloudflared) signaling + IPv6 TURN. Bore is signaling-only last resort.
# Never put TURN on bore — TCP tunnels break UDP relays and starve video (~7 fps).

couchlink_windows_run_dir() {
  local win_user=""
  if command -v cmd.exe >/dev/null 2>&1; then
    win_user="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r')"
  fi
  win_user="${win_user:-josep}"
  echo "/mnt/c/Users/${win_user}/AppData/Local/couchlink-run"
}

# Global unicast IPv6 written by enable-upnp.ps1 (Windows Wi-Fi), or queried live.
couchlink_read_public_ipv6() {
  local f v6
  f="$(couchlink_windows_run_dir)/public-ipv6.txt"
  if [[ -f "$f" ]]; then
    v6="$(tr -d ' \r\n' <"$f")"
    if [[ "$v6" =~ ^[23] ]]; then
      printf '%s' "$v6"
      return 0
    fi
  fi
  if command -v powershell.exe >/dev/null 2>&1; then
    v6="$(powershell.exe -NoProfile -Command \
      "(Get-NetIPAddress -AddressFamily IPv6 | Where-Object { \$_.AddressState -eq 'Preferred' -and \$_.InterfaceAlias -notmatch 'WSL|vEthernet|Loopback|Bluetooth' -and \$_.IPAddress -match '^[23]' -and \$_.IPAddress -notlike 'fd*' } | Sort-Object @{e={ if (\$_.PrefixOrigin -eq 'Dhcp') {0} elseif (\$_.SuffixOrigin -eq 'Link') {1} else {2} }} | Select-Object -First 1 -ExpandProperty IPAddress)" \
      2>/dev/null | tr -d ' \r\n')"
    if [[ "$v6" =~ ^[23] ]]; then
      printf '%s' "$v6"
      return 0
    fi
  fi
  return 1
}

# Bracket IPv6 for URLs; leave IPv4 / hostnames alone.
couchlink_bracket_host() {
  local h="$1"
  if [[ "$h" == *:* && "$h" != \[* ]]; then
    printf '[%s]' "$h"
  else
    printf '%s' "$h"
  fi
}

couchlink_ensure_cloudflared() {
  local root="$1"
  local bin="$root/.tools/cloudflared"
  if [[ -x "$bin" ]]; then
    printf '%s' "$bin"
    return 0
  fi
  mkdir -p "$root/.tools"
  echo "==> downloading cloudflared (HTTPS invite — unlocks browser WebCodecs)" >&2
  if ! curl -fsSL -o "$bin" --max-time 90 \
    "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64"; then
    return 1
  fi
  chmod +x "$bin"
  printf '%s' "$bin"
}

# Quick tunnel → https://*.trycloudflare.com (secure context for WebCodecs).
# Sets COUCHLINK_CF_URL and appends PID to COUCHLINK_TUNNEL_PIDS.
couchlink_start_cloudflared() {
  local root="$1"
  local local_port="${2:-8443}"
  local cf
  cf="$(couchlink_ensure_cloudflared "$root")" || return 1

  local log
  log="$(mktemp /tmp/couchlink-cloudflared.XXXXXX.log)"
  "$cf" tunnel --url "http://127.0.0.1:${local_port}" --no-autoupdate >"$log" 2>&1 &
  local pid=$!

  local i url=""
  for i in $(seq 1 40); do
    url="$(grep -oE 'https://[a-zA-Z0-9.-]+\.trycloudflare\.com' "$log" 2>/dev/null | head -1 || true)"
    if [[ -n "$url" ]]; then
      break
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "==> cloudflared exited early:" >&2
      tail -15 "$log" >&2 || true
      return 1
    fi
    sleep 0.4
  done

  if [[ -z "$url" ]]; then
    echo "==> cloudflared timed out waiting for trycloudflare URL" >&2
    kill "$pid" 2>/dev/null || true
    return 1
  fi

  declare -ga COUCHLINK_TUNNEL_PIDS=("${COUCHLINK_TUNNEL_PIDS[@]:-}" "$pid")
  export COUCHLINK_CF_URL="$url"
  echo "==> cloudflared HTTPS invite: $url"
  return 0
}

couchlink_ensure_bore() {
  local root="$1"
  local bin="$root/.tools/bore"
  if [[ -x "$bin" ]]; then
    printf '%s' "$bin"
    return 0
  fi
  mkdir -p "$root/.tools"
  local url="https://github.com/ekzhang/bore/releases/download/v0.6.0/bore-v0.6.0-x86_64-unknown-linux-musl.tar.gz"
  echo "==> downloading bore (signaling-only fallback)" >&2
  if ! curl -fsSL -o /tmp/couchlink-bore.tgz --max-time 60 "$url"; then
    return 1
  fi
  tar -xzf /tmp/couchlink-bore.tgz -C "$root/.tools" bore
  chmod +x "$bin"
  printf '%s' "$bin"
}

# Signaling-only bore tunnel (never TURN — UDP relays break through TCP bore).
# Sets COUCHLINK_BORE_SIG_PORT; appends PID to COUCHLINK_TUNNEL_PIDS.
couchlink_start_bore_signaling() {
  local root="$1"
  local sig_port="${2:-8443}"
  local bore
  bore="$(couchlink_ensure_bore "$root")" || return 1

  local sig_log
  sig_log="$(mktemp /tmp/couchlink-bore-sig.XXXXXX.log)"
  "$bore" local "$sig_port" --to bore.pub >"$sig_log" 2>&1 &
  local sig_pid=$!

  local i remote_sig=""
  for i in $(seq 1 25); do
    remote_sig="$(grep -oE 'bore\.pub:[0-9]+' "$sig_log" 2>/dev/null | head -1 | cut -d: -f2 || true)"
    if [[ -n "$remote_sig" ]]; then
      break
    fi
    if ! kill -0 "$sig_pid" 2>/dev/null; then
      echo "==> bore signaling exited early:" >&2
      tail -5 "$sig_log" >&2 || true
      return 1
    fi
    sleep 0.4
  done

  if [[ -z "$remote_sig" ]]; then
    echo "==> bore signaling timed out" >&2
    kill "$sig_pid" 2>/dev/null || true
    return 1
  fi

  declare -ga COUCHLINK_TUNNEL_PIDS=("${COUCHLINK_TUNNEL_PIDS[@]:-}" "$sig_pid")
  # Back-compat name used by older cleanup snippets.
  declare -ga COUCHLINK_BORE_PIDS=("$sig_pid")
  export COUCHLINK_BORE_SIG_PORT="$remote_sig"
  echo "==> bore signaling only: http://bore.pub:${remote_sig} (TURN stays on real IP/IPv6)"
  return 0
}
