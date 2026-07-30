# Sourced helper: best-effort automatic router port-forwarding via UPnP IGD
# (miniupnpc). Returns non-zero when the mapping fails so callers can react.

upnp_local_ip() {
  # Prefer shared detector (Linux + macOS); fall back to hostname -I.
  if declare -F couchlink_local_ip >/dev/null 2>&1; then
    couchlink_local_ip
    return 0
  fi
  local _root
  _root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  # shellcheck disable=SC1091
  if [[ -f "$_root/scripts/lib-platform.sh" ]]; then
    # shellcheck source=lib-platform.sh
    source "$_root/scripts/lib-platform.sh"
    couchlink_local_ip
    return 0
  fi
  hostname -I 2>/dev/null | awk '{print $1}'
}

# upnp_open <port> <tcp|udp> <description> [optional local IP]
# Returns 0 on success, 1 on failure.
upnp_open() {
  local port="$1" proto="$2" desc="$3" ip="${4:-}"
  command -v upnpc >/dev/null || { echo "upnpc not installed — skipping UPnP for $desc ($port/$proto)"; return 1; }
  if [[ -z "$ip" ]]; then
    ip="$(upnp_local_ip)"
  fi
  [[ -z "$ip" ]] && return 1
  # Routers sometimes hang on UPnP — never block session start longer than a few seconds.
  if timeout 5 upnpc -e "couchlink-$desc" -a "$ip" "$port" "$port" "$proto" >/tmp/couchlink-upnp.log 2>&1; then
    echo "==> UPnP: opened $port/$proto on router → $ip ($desc)"
    return 0
  fi
  echo "==> UPnP: router didn't accept $port/$proto ($desc)"
  return 1
}

# upnp_close <port> <tcp|udp>
upnp_close() {
  local port="$1" proto="$2"
  command -v upnpc >/dev/null || return 0
  timeout 3 upnpc -d "$port" "$proto" >/dev/null 2>&1 || true
}
