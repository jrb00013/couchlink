# Sourced by run.sh — outbound reachability when router UPnP is unavailable
# AND no Tailscale/WireGuard mesh is up (mesh is tried first in run.sh).
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

# True when *this* machine actually holds `addr` on a global-scope interface.
#
# couchlink_read_public_ipv6 deliberately returns the *Windows* address, which is
# right for a TCP invite (netsh portproxy forwards v6->WSL) and wrong for TURN.
# coturn runs inside WSL, which has no IPv6 at all in NAT mode, and portproxy
# cannot forward UDP — so advertising turn:[windows-v6] hands the friend a relay
# that can never answer. Their browser gathers no `typ relay` candidate, and ICE
# fails outright the moment their NAT refuses the direct path. Silent, and
# permanent: it looks exactly like the friend's network being at fault.
couchlink_owns_ipv6() {
  local addr="${1:-}"
  [[ -n "$addr" ]] || return 1
  ip -6 addr show scope global 2>/dev/null | grep -qiF "$addr"
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

# Persistent state for the active quick tunnel + watchdog.
# Quick tunnels (trycloudflare.com) have no uptime guarantee — we keep a
# watchdog that restarts cloudflared when the process dies or the hostname
# goes NXDOMAIN, and rewrites the friend join URL onto the new host.
couchlink_cf_state_dir() {
  printf '%s' "${COUCHLINK_CF_STATE_DIR:-/tmp/couchlink-cf}"
}

couchlink_cf_join_file() {
  printf '%s' "${COUCHLINK_JOIN_URL_FILE:-/tmp/couchlink-join-url.txt}"
}

# True when the trycloudflare edge still resolves and accepts TCP/TLS.
# Origin 5xx is fine (signaling blip) — NXDOMAIN / connect failure is not.
couchlink_cf_edge_ok() {
  local url="$1"
  local host code
  [[ -n "$url" ]] || return 1
  host="${url#https://}"
  host="${host%%/*}"
  if command -v getent >/dev/null 2>&1; then
    getent hosts "$host" >/dev/null 2>&1 || return 1
  fi
  code="$(curl -sS -o /dev/null -w '%{http_code}' --connect-timeout 5 --max-time 8 \
    -A 'couchlink-cf-watchdog' "${url}/" 2>/dev/null || true)"
  [[ -n "$code" && "$code" != "000" ]]
}

# Swap every *.trycloudflare.com host in the published join URL onto `new_base`.
couchlink_cf_rewrite_join_hosts() {
  local new_base="$1"
  local join_file new_host join
  join_file="$(couchlink_cf_join_file)"
  [[ -f "$join_file" ]] || return 0
  new_host="${new_base#https://}"
  new_host="${new_host%%/*}"
  [[ -n "$new_host" ]] || return 0
  join="$(tr -d '\r\n' <"$join_file")"
  [[ "$join" == *trycloudflare.com* ]] || return 0
  join="$(printf '%s' "$join" | sed -E \
    -e "s|https://[A-Za-z0-9.-]+\\.trycloudflare\\.com|https://${new_host}|g" \
    -e "s|wss://[A-Za-z0-9.-]+\\.trycloudflare\\.com|wss://${new_host}|g" \
    -e "s|wss%3A%2F%2F[A-Za-z0-9.-]+\\.trycloudflare\\.com|wss%3A%2F%2F${new_host}|g")"
  printf '%s\n' "$join" >"$join_file"
  echo "==> cloudflared restarted — new friend join URL:" >&2
  echo "$join" >&2
  if command -v clip.exe >/dev/null 2>&1; then
    printf '%s' "$join" | clip.exe 2>/dev/null || true
  fi
}

# Spawn one cloudflared quick tunnel. Does not start the watchdog.
# Sets COUCHLINK_CF_URL / COUCHLINK_CF_PID and records state under couchlink_cf_state_dir.
couchlink_spawn_cloudflared() {
  local root="$1"
  local local_port="${2:-8443}"
  local cf state log pid url i
  local ha_conn retries
  cf="$(couchlink_ensure_cloudflared "$root")" || return 1

  state="$(couchlink_cf_state_dir)"
  mkdir -p "$state"
  log="$(mktemp /tmp/couchlink-cloudflared.XXXXXX.log)"

  # Quick tunnels force ha-connections=1 on Cloudflare's side (flag is accepted
  # but ignored — see cloudflared Settings log). retries still helps; the
  # watchdog below is what recovers from death / NXDOMAIN.
  ha_conn="${COUCHLINK_CF_HA_CONNECTIONS:-4}"
  retries="${COUCHLINK_CF_RETRIES:-15}"

  "$cf" tunnel \
    --url "http://127.0.0.1:${local_port}" \
    --ha-connections "$ha_conn" \
    --retries "$retries" \
    --no-autoupdate \
    >"$log" 2>&1 &
  pid=$!

  url=""
  for i in $(seq 1 50); do
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

  printf '%s\n' "$pid" >"$state/pid"
  printf '%s\n' "$url" >"$state/url"
  printf '%s\n' "$log" >"$state/log"
  printf '%s\n' "$local_port" >"$state/port"
  rm -f "$state/stop"

  declare -ga COUCHLINK_TUNNEL_PIDS=("${COUCHLINK_TUNNEL_PIDS[@]:-}" "$pid")
  export COUCHLINK_CF_URL="$url"
  export COUCHLINK_CF_PID="$pid"
  echo "==> cloudflared HTTPS invite: $url"
  return 0
}

# Restart loop: if cloudflared dies or the trycloudflare hostname NXDOMAINs,
# spawn a fresh quick tunnel and rewrite the published join URL onto it.
couchlink_watch_cloudflared() {
  local root="$1"
  local local_port="${2:-8443}"
  local state pid url fail
  state="$(couchlink_cf_state_dir)"
  # Disown from job control noise; run.sh tracks us via COUCHLINK_TUNNEL_PIDS.
  while true; do
    sleep "${COUCHLINK_CF_WATCH_SECS:-20}"
    [[ -f "$state/stop" ]] && exit 0

    # If signaling is gone, the session is over — exit quietly.
    if ! timeout 0.25 bash -c "echo >/dev/tcp/127.0.0.1/${local_port}" 2>/dev/null; then
      fail=0
      for _ in 1 2 3; do
        sleep 2
        if timeout 0.25 bash -c "echo >/dev/tcp/127.0.0.1/${local_port}" 2>/dev/null; then
          fail=0
          break
        fi
        fail=1
      done
      [[ "$fail" == "1" ]] && exit 0
    fi

    pid="$(tr -d ' \r\n' <"$state/pid" 2>/dev/null || true)"
    url="$(tr -d ' \r\n' <"$state/url" 2>/dev/null || true)"
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null && couchlink_cf_edge_ok "$url"; then
      continue
    fi

    echo "==> cloudflared dead or unreachable (pid=${pid:-none} url=${url:-none}) — restarting" >&2
    if [[ -n "$pid" ]]; then
      kill "$pid" 2>/dev/null || true
      sleep 0.5
      kill -9 "$pid" 2>/dev/null || true
    fi

    if ! couchlink_spawn_cloudflared "$root" "$local_port"; then
      echo "==> cloudflared restart failed — retrying in 30s" >&2
      sleep 30
      continue
    fi
    export COUCHLINK_INVITE_SIGNALING="${COUCHLINK_CF_URL/https:/wss:}/ws"
    couchlink_cf_rewrite_join_hosts "$COUCHLINK_CF_URL"
  done
}

# Quick tunnel → https://*.trycloudflare.com (secure context for WebCodecs).
# Sets COUCHLINK_CF_URL and appends PIDs (cloudflared + watchdog) to COUCHLINK_TUNNEL_PIDS.
couchlink_start_cloudflared() {
  local root="$1"
  local local_port="${2:-8443}"
  local state wpid

  couchlink_spawn_cloudflared "$root" "$local_port" || return 1

  if [[ "${COUCHLINK_CF_WATCHDOG:-1}" != "1" ]]; then
    return 0
  fi

  state="$(couchlink_cf_state_dir)"
  couchlink_watch_cloudflared "$root" "$local_port" &
  wpid=$!
  printf '%s\n' "$wpid" >"$state/watchdog.pid"
  declare -ga COUCHLINK_TUNNEL_PIDS=("${COUCHLINK_TUNNEL_PIDS[@]:-}" "$wpid")
  echo "==> cloudflared keepalive watchdog pid=$wpid (restarts on death/NXDOMAIN)"
  return 0
}

couchlink_stop_cloudflared_watchdog() {
  local state wpid pid
  state="$(couchlink_cf_state_dir)"
  mkdir -p "$state"
  : >"$state/stop"
  wpid="$(tr -d ' \r\n' <"$state/watchdog.pid" 2>/dev/null || true)"
  pid="$(tr -d ' \r\n' <"$state/pid" 2>/dev/null || true)"
  [[ -n "$wpid" ]] && kill "$wpid" 2>/dev/null || true
  [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
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
