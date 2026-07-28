# Sourced helper: best-effort automatic router port-forwarding via UPnP IGD
# (miniupnpc). No-op with a warning if the router doesn't support UPnP —
# falls back to whatever ports are already reachable.

upnp_local_ip() {
  hostname -I 2>/dev/null | awk '{print $1}'
}

# upnp_open <port> <tcp|udp> <description>
upnp_open() {
  local port="$1" proto="$2" desc="$3"
  command -v upnpc >/dev/null || { echo "upnpc not installed — skipping UPnP for $desc ($port/$proto)"; return 0; }
  local ip
  ip="$(upnp_local_ip)"
  [[ -z "$ip" ]] && return 0
  if upnpc -e "couchlink-$desc" -a "$ip" "$port" "$port" "$proto" >/tmp/couchlink-upnp.log 2>&1; then
    echo "==> UPnP: opened $port/$proto on router ($desc)"
  else
    echo "==> UPnP: router didn't accept $port/$proto ($desc) — forward it manually if friends can't connect"
  fi
}

# upnp_close <port> <tcp|udp>
upnp_close() {
  local port="$1" proto="$2"
  command -v upnpc >/dev/null || return 0
  upnpc -d "$port" "$proto" >/dev/null 2>&1 || true
}
