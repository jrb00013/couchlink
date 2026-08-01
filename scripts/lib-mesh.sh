# Sourced by run.sh — Tailscale / WireGuard as the prime online path.
# When a mesh is already up, invite looks like LAN (no public TURN / Cloudflare).
# Fallback chain (UPnP → cloudflared → IPv6 → bore) stays in run.sh / lib-online-tunnel.sh.

# Prints Tailscale IPv4 if the daemon is up and has an address; else fails.
couchlink_tailscale_ip() {
  local ip="" bin=""
  if command -v tailscale >/dev/null 2>&1; then
    bin="$(command -v tailscale)"
  elif command -v tailscale.exe >/dev/null 2>&1; then
    bin="$(command -v tailscale.exe)"
  else
    local cand
    for cand in \
      "/mnt/c/Program Files/Tailscale/tailscale.exe" \
      "/mnt/c/Program Files (x86)/Tailscale/tailscale.exe"; do
      if [[ -x "$cand" ]]; then
        bin="$cand"
        break
      fi
    done
  fi
  [[ -n "$bin" ]] || return 1

  ip="$("$bin" ip -4 2>/dev/null | head -1 | tr -d ' \r\n' || true)"
  if [[ "$ip" =~ ^100\. ]]; then
    printf '%s' "$ip"
    return 0
  fi
  return 1
}

# Prints WireGuard couchlink address (default 10.66.0.1) if wg0 (or COUCHLINK_WG_IF) is up.
couchlink_wireguard_ip() {
  local ifc="${COUCHLINK_WG_IF:-wg0}"
  local expect="${COUCHLINK_WG_HOST_IP:-10.66.0.1}"
  local ip=""

  if command -v wg >/dev/null 2>&1; then
    if ! wg show "$ifc" >/dev/null 2>&1; then
      return 1
    fi
  elif [[ ! -d "/sys/class/net/$ifc" ]]; then
    return 1
  fi

  if command -v ip >/dev/null 2>&1; then
    ip="$(ip -4 -o addr show dev "$ifc" 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | head -1 || true)"
  fi
  if [[ -z "$ip" ]] && command -v ifconfig >/dev/null 2>&1; then
    ip="$(ifconfig "$ifc" 2>/dev/null | awk '/inet /{print $2; exit}' | tr -d 'addr:' || true)"
  fi

  # Windows WireGuard: interface may live on Windows while couchlink is in WSL —
  # honor an explicit override so host invite still advertises the mesh IP.
  if [[ -z "$ip" && -n "${COUCHLINK_WG_HOST_IP:-}" ]]; then
    ip="$COUCHLINK_WG_HOST_IP"
  fi

  if [[ -n "$ip" ]]; then
    printf '%s' "$ip"
    return 0
  fi
  # Interface up but address unknown — still advertise the conventional host IP.
  if command -v wg >/dev/null 2>&1 && wg show "$ifc" >/dev/null 2>&1; then
    printf '%s' "$expect"
    return 0
  fi
  return 1
}

# Apply LAN-style invite on a mesh IP. No TURN. Host still dials loopback.
# Sets COUCHLINK_MESH to tailscale|wireguard. Returns 0 on success.
couchlink_apply_mesh_invite() {
  local kind="$1"
  local mesh_ip="$2"
  local port="${3:-8443}"

  [[ -n "$mesh_ip" ]] || return 1

  export COUCHLINK_SIGNALING="ws://127.0.0.1:${port}/ws"
  export COUCHLINK_INVITE_SIGNALING="ws://${mesh_ip}:${port}/ws"
  unset COUCHLINK_TURN_URL || true
  unset COUCHLINK_TURN_EXTERNAL_IP || true
  export COUCHLINK_MESH="$kind"
  export COUCHLINK_MESH_IP="$mesh_ip"

  echo "==> PRIME mesh ($kind) — join via http://${mesh_ip}:${port}/ (no public TURN / Cloudflare)"
  case "$kind" in
    tailscale)
      echo "    friend must be on your Tailscale tailnet (same account / shared node)"
      echo "    native client preferred; browser WebCodecs wants https (use native or SSH tunnel)"
      ;;
    wireguard)
      echo "    friend imports infra/wireguard/wg0-player.conf then: wg-quick up wg0"
      echo "    native client preferred; browser WebCodecs wants https"
      ;;
  esac
  return 0
}

# Prefer Tailscale, then WireGuard. Skip with COUCHLINK_SKIP_MESH=1.
# Returns 0 if a mesh invite was applied.
couchlink_try_mesh_online() {
  local port="${1:-8443}"
  [[ "${COUCHLINK_SKIP_MESH:-0}" == "1" ]] && return 1

  # Explicit override (tests / Windows WG + WSL host).
  if [[ -n "${COUCHLINK_MESH:-}" && -n "${COUCHLINK_MESH_IP:-}" ]]; then
    couchlink_apply_mesh_invite "$COUCHLINK_MESH" "$COUCHLINK_MESH_IP" "$port"
    return $?
  fi

  local prefer="${COUCHLINK_MESH_PREFER:-tailscale,wireguard}"
  local part ip
  IFS=',' read -ra _mesh_order <<<"$prefer"
  for part in "${_mesh_order[@]}"; do
    part="$(echo "$part" | tr -d '[:space:]')"
    case "$part" in
      tailscale)
        ip="$(couchlink_tailscale_ip 2>/dev/null || true)"
        if [[ -n "$ip" ]]; then
          couchlink_apply_mesh_invite tailscale "$ip" "$port"
          return 0
        fi
        ;;
      wireguard)
        ip="$(couchlink_wireguard_ip 2>/dev/null || true)"
        if [[ -n "$ip" ]]; then
          couchlink_apply_mesh_invite wireguard "$ip" "$port"
          return 0
        fi
        ;;
    esac
  done

  echo "==> no Tailscale / WireGuard mesh up — falling back to UPnP / Cloudflare / IPv6"
  echo "    setup: ./scripts/setup-tailscale.sh   or   ./scripts/setup-wireguard.sh"
  echo "    docs:  docs/WIREGUARD.md  ·  docs/MESH.md"
  return 1
}
