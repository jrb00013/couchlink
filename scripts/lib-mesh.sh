# Sourced by run.sh — Tailscale / WireGuard as the prime online path.
# When a mesh is already up, invite uses the mesh IP (no Cloudflare).
# On WSL, WebRTC UDP still needs TURN advertised on the mesh IP (portproxy 3478).
# Fallback chain (UPnP → cloudflared → IPv6 → bore) stays in run.sh / lib-online-tunnel.sh.

couchlink_find_tailscale_bin() {
  if command -v tailscale >/dev/null 2>&1; then
    command -v tailscale
    return 0
  fi
  if command -v tailscale.exe >/dev/null 2>&1; then
    command -v tailscale.exe
    return 0
  fi
  local cand
  for cand in \
    "/mnt/c/Program Files/Tailscale/tailscale.exe" \
    "/mnt/c/Program Files (x86)/Tailscale/tailscale.exe" \
    "/mnt/c/Users/${USER}/AppData/Local/Tailscale/tailscale.exe"; do
    if [[ -x "$cand" || -f "$cand" ]]; then
      printf '%s' "$cand"
      return 0
    fi
  done
  # Windows username may differ from WSL user.
  if command -v cmd.exe >/dev/null 2>&1; then
    local win_user
    win_user="$(cmd.exe /c "echo %USERNAME%" 2>/dev/null | tr -d '\r')"
    cand="/mnt/c/Users/${win_user}/AppData/Local/Tailscale/tailscale.exe"
    if [[ -f "$cand" ]]; then
      printf '%s' "$cand"
      return 0
    fi
  fi
  return 1
}

couchlink_find_wg_exe() {
  if command -v wg.exe >/dev/null 2>&1; then
    command -v wg.exe
    return 0
  fi
  local cand="/mnt/c/Program Files/WireGuard/wg.exe"
  if [[ -f "$cand" ]]; then
    printf '%s' "$cand"
    return 0
  fi
  return 1
}

# Prints Tailscale IPv4 if the daemon is up and has an address; else fails.
couchlink_tailscale_ip() {
  local ip="" bin=""
  bin="$(couchlink_find_tailscale_bin 2>/dev/null || true)"
  [[ -n "$bin" ]] || return 1

  ip="$("$bin" ip -4 2>/dev/null | head -1 | tr -d ' \r\n' || true)"
  if [[ "$ip" =~ ^100\. ]]; then
    printf '%s' "$ip"
    return 0
  fi
  # Fallback: parse status for this node's Tailscale IP.
  ip="$("$bin" status --self --json 2>/dev/null | tr -d '\r' \
    | grep -oE '"TailscaleIPs"[[:space:]]*:[[:space:]]*\[[^]]+' \
    | grep -oE '100\.[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)"
  if [[ "$ip" =~ ^100\. ]]; then
    printf '%s' "$ip"
    return 0
  fi
  return 1
}

# WireGuard IPv4 on Windows (tunnel lives outside WSL).
couchlink_windows_wireguard_ip() {
  local expect="${COUCHLINK_WG_HOST_IP:-10.66.0.1}"
  local wgexe ip=""
  wgexe="$(couchlink_find_wg_exe 2>/dev/null || true)"
  if [[ -n "$wgexe" ]]; then
    # Any active tunnel is enough signal; prefer the couchlink host address if present.
    if "$wgexe" show interfaces >/dev/null 2>&1; then
      local ifaces
      ifaces="$("$wgexe" show interfaces 2>/dev/null | tr -d '\r' | tr ' ' '\n' | grep -v '^$' || true)"
      if [[ -n "$ifaces" ]]; then
        # Prefer configured couchlink host IP if assigned on a WireGuard adapter.
        if command -v powershell.exe >/dev/null 2>&1; then
          ip="$(powershell.exe -NoProfile -Command \
            "Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue | Where-Object { \$_.InterfaceAlias -match 'WireGuard|Wintun|wg0' -and \$_.IPAddress -eq '${expect}' } | Select-Object -First 1 -ExpandProperty IPAddress" \
            2>/dev/null | tr -d ' \r\n')"
          if [[ -z "$ip" ]]; then
            ip="$(powershell.exe -NoProfile -Command \
              "Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue | Where-Object { \$_.InterfaceAlias -match 'WireGuard|Wintun' -and \$_.IPAddress -notlike '169.254*' } | Select-Object -First 1 -ExpandProperty IPAddress" \
              2>/dev/null | tr -d ' \r\n')"
          fi
        fi
        if [[ -z "$ip" ]]; then
          ip="$expect"
        fi
        printf '%s' "$ip"
        return 0
      fi
    fi
  fi
  return 1
}

# Prints WireGuard couchlink address (default 10.66.0.1) if wg0 (or COUCHLINK_WG_IF) is up.
couchlink_wireguard_ip() {
  local ifc="${COUCHLINK_WG_IF:-wg0}"
  local expect="${COUCHLINK_WG_HOST_IP:-10.66.0.1}"
  local ip="" wsl_mode=""

  # Explicit override always wins (Windows WG + WSL host).
  if [[ "${COUCHLINK_WG_FORCE:-0}" == "1" && -n "${COUCHLINK_WG_HOST_IP:-}" ]]; then
    printf '%s' "$COUCHLINK_WG_HOST_IP"
    return 0
  fi

  # WSL without mirrored networking cannot host inbound WireGuard: NAT mode has
  # no global IPv6 and netsh portproxy is TCP-only, so a WSL wg0 is unreachable
  # from the friend. Prefer the Windows WireGuard app tunnel, which owns the
  # real addresses. Only fall back to WSL wg0 when mirroring is live.
  if declare -F couchlink_wsl_networking_mode >/dev/null 2>&1; then
    wsl_mode="$(couchlink_wsl_networking_mode 2>/dev/null || true)"
  fi
  if [[ "$wsl_mode" == "nat" ]]; then
    if ip="$(couchlink_windows_wireguard_ip 2>/dev/null)"; then
      printf '%s' "$ip"
      return 0
    fi
    return 1
  fi

  if command -v wg >/dev/null 2>&1 && wg show "$ifc" >/dev/null 2>&1; then
    if command -v ip >/dev/null 2>&1; then
      ip="$(ip -4 -o addr show dev "$ifc" 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | head -1 || true)"
    fi
    if [[ -z "$ip" ]] && command -v ifconfig >/dev/null 2>&1; then
      ip="$(ifconfig "$ifc" 2>/dev/null | awk '/inet /{print $2; exit}' | tr -d 'addr:' || true)"
    fi
    printf '%s' "${ip:-$expect}"
    return 0
  fi

  if couchlink_windows_wireguard_ip >/dev/null 2>&1; then
    couchlink_windows_wireguard_ip
    return 0
  fi

  return 1
}

# Apply mesh invite. Sets COUCHLINK_MESH / COUCHLINK_MESH_IP / ICE.
# On WSL sets COUCHLINK_MESH_NEED_TURN=1 and TURN URL on the mesh IP (UDP via portproxy).
# On native Linux/macOS clears TURN (direct host candidates on the mesh iface).
couchlink_apply_mesh_invite() {
  local kind="$1"
  local mesh_ip="$2"
  local port="${3:-8443}"
  local platform="${4:-}"

  [[ -n "$mesh_ip" ]] || return 1
  if [[ -z "$platform" ]]; then
    # shellcheck disable=SC1091
    if declare -F couchlink_detect_platform >/dev/null 2>&1; then
      platform="$(couchlink_detect_platform)"
    else
      platform="linux"
    fi
  fi

  export COUCHLINK_SIGNALING="ws://127.0.0.1:${port}/ws"
  export COUCHLINK_INVITE_SIGNALING="ws://${mesh_ip}:${port}/ws"
  export COUCHLINK_MESH="$kind"
  export COUCHLINK_MESH_IP="$mesh_ip"

  if [[ "$platform" == "wsl" ]]; then
    # Ephemeral WebRTC UDP is not portproxied — TURN on the mesh IP carries media.
    # Do NOT stuff mesh_ip into COUCHLINK_ICE_IPS alongside the WSL LAN IP: webrtc-ice
    # allows only one sole IPv4 NAT mapping and used to crash the host on player join.
    export COUCHLINK_TURN_URL="turn:${mesh_ip}:3478"
    export COUCHLINK_TURN_EXTERNAL_IP="$mesh_ip"
    export COUCHLINK_MESH_NEED_TURN=1
    if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
      echo "==> PRIME mesh ($kind) — join via http://${mesh_ip}:${port}/"
      echo "    WSL: TURN on ${mesh_ip}:3478 (UDP via Windows portproxy); ICE keeps existing ICE_IPS"
    fi
  else
    # Native: mesh iface is local — advertise it as the sole host candidate IP.
    export COUCHLINK_ICE_IPS="$mesh_ip"
    unset COUCHLINK_TURN_URL || true
    unset COUCHLINK_TURN_EXTERNAL_IP || true
    export COUCHLINK_MESH_NEED_TURN=0
    if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
      echo "==> PRIME mesh ($kind) — join via http://${mesh_ip}:${port}/ (no public Cloudflare)"
    fi
  fi

  if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
    case "$kind" in
      headscale)
        echo "    Friend: ./install.sh --online  → paste this join URL (auto-joins Headscale; no Tailscale Inc account)"
        echo "    Optional: ./install.sh --online --unblock-firewall"
        ;;
      tailscale)
        echo "    Friend: COUCHLINK_INSTALL_TAILSCALE_CLOUD=1 ./install.sh && ./install.sh --online"
        echo "            (optional Tailscale Inc cloud fallback — not required for Headscale)"
        ;;
      wireguard)
        echo "    friend imports infra/wireguard/wg0-player.conf then brings WireGuard up"
        echo "    (prefer Headscale: ./install.sh --host --online + friend ./install.sh --online)"
        ;;
    esac
  fi
  return 0
}

# Persist mesh invite env for parent shells (enable-*.sh runs as a subprocess).
couchlink_write_mesh_env_file() {
  local out="$1"
  {
    echo "# generated by couchlink — do not commit"
    [[ -n "${COUCHLINK_MESH:-}" ]] && echo "export COUCHLINK_MESH=$(printf '%q' "$COUCHLINK_MESH")"
    [[ -n "${COUCHLINK_MESH_IP:-}" ]] && echo "export COUCHLINK_MESH_IP=$(printf '%q' "$COUCHLINK_MESH_IP")"
    [[ -n "${COUCHLINK_HS_URL:-}" ]] && echo "export COUCHLINK_HS_URL=$(printf '%q' "$COUCHLINK_HS_URL")"
    [[ -n "${COUCHLINK_TS_AUTHKEY:-}" ]] && echo "export COUCHLINK_TS_AUTHKEY=$(printf '%q' "$COUCHLINK_TS_AUTHKEY")"
    [[ -n "${COUCHLINK_SIGNALING:-}" ]] && echo "export COUCHLINK_SIGNALING=$(printf '%q' "$COUCHLINK_SIGNALING")"
    [[ -n "${COUCHLINK_INVITE_SIGNALING:-}" ]] && echo "export COUCHLINK_INVITE_SIGNALING=$(printf '%q' "$COUCHLINK_INVITE_SIGNALING")"
    [[ -n "${COUCHLINK_TURN_URL:-}" ]] && echo "export COUCHLINK_TURN_URL=$(printf '%q' "$COUCHLINK_TURN_URL")"
    [[ -n "${COUCHLINK_TURN_EXTERNAL_IP:-}" ]] && echo "export COUCHLINK_TURN_EXTERNAL_IP=$(printf '%q' "$COUCHLINK_TURN_EXTERNAL_IP")"
    [[ -n "${COUCHLINK_ICE_IPS:-}" ]] && echo "export COUCHLINK_ICE_IPS=$(printf '%q' "$COUCHLINK_ICE_IPS")"
    echo "export COUCHLINK_MESH_NEED_TURN=${COUCHLINK_MESH_NEED_TURN:-0}"
  } >"$out"
}

# Friend: if invite/env has hs= + tskey=, join Headscale headlessly.
couchlink_try_client_headscale_join() {
  local root="${1:-}"
  [[ -n "$root" ]] || return 1
  [[ "${COUCHLINK_SKIP_MESH:-0}" == "1" ]] && return 0

  local hs="${COUCHLINK_HS_URL:-}"
  local key="${COUCHLINK_TS_AUTHKEY:-}"
  local join="${COUCHLINK_JOIN_URL:-}"

  if [[ (-z "$hs" || -z "$key") && -n "$join" ]]; then
    if [[ -z "$hs" ]]; then
      hs="$(printf '%s' "$join" | python3 -c '
import sys, urllib.parse
u = urllib.parse.urlparse(sys.stdin.read().strip())
q = urllib.parse.parse_qs(u.query)
print((q.get("hs") or [""])[0])
' 2>/dev/null || true)"
    fi
    if [[ -z "$key" ]]; then
      key="$(printf '%s' "$join" | python3 -c '
import sys, urllib.parse
u = urllib.parse.urlparse(sys.stdin.read().strip())
q = urllib.parse.parse_qs(u.query)
print((q.get("tskey") or [""])[0])
' 2>/dev/null || true)"
    fi
  fi

  [[ -n "$hs" && -n "$key" ]] || return 1
  export COUCHLINK_HS_URL="$hs"
  export COUCHLINK_TS_AUTHKEY="$key"
  echo "==> Headscale invite detected — joining control plane…"
  bash "$root/scripts/join-headscale.sh" "$hs" "$key"
}

# Prefer Headscale (when env already set / enable wrote mesh.env), then Tailscale, then WireGuard.
# Skip with COUCHLINK_SKIP_MESH=1. Returns 0 if a mesh invite was applied.

# Friend/client: optional Tailscale Inc cloud path (SUPPRESSED by default).
# Kept for fallback: COUCHLINK_ENSURE_TAILSCALE_CLOUD=1 ./scripts/run.sh client --online
# Headscale joins use join-headscale.sh / ensure-headscale-client.sh instead.
couchlink_ensure_client_tailscale() {
  [[ "${COUCHLINK_SKIP_MESH:-0}" == "1" ]] && return 0
  local root="${1:-}"
  local ip=""
  if ip="$(couchlink_tailscale_ip 2>/dev/null)"; then
    echo "==> mesh client ready ($ip)"
    return 0
  fi
  # Do NOT auto-run setup-tailscale.sh — that pops Windows Tailscale / UAC / login.tailscale.com.
  if [[ "${COUCHLINK_ENSURE_TAILSCALE_CLOUD:-0}" != "1" ]]; then
    echo "==> no Tailscale Inc mesh (ok — Headscale paste-link does not need it)"
    echo "    optional cloud fallback: COUCHLINK_ENSURE_TAILSCALE_CLOUD=1 + ./scripts/setup-tailscale.sh --ensure"
    return 1
  fi
  echo "==> COUCHLINK_ENSURE_TAILSCALE_CLOUD=1 — ensuring Tailscale Inc client…"
  if [[ -n "$root" && -x "$root/scripts/setup-tailscale.sh" ]]; then
    bash "$root/scripts/setup-tailscale.sh" --ensure || true
    if ip="$(couchlink_tailscale_ip 2>/dev/null)"; then
      echo "==> Tailscale cloud ready ($ip)"
      return 0
    fi
  fi
  return 1
}

couchlink_try_mesh_online() {
  local port="${1:-8443}"
  local platform="${2:-}"
  [[ "${COUCHLINK_SKIP_MESH:-0}" == "1" ]] && return 1

  if [[ -z "$platform" ]] && declare -F couchlink_detect_platform >/dev/null 2>&1; then
    platform="$(couchlink_detect_platform)"
  fi

  # Explicit override (tests / enable-headscale mesh.env / Windows WG + WSL host).
  if [[ -n "${COUCHLINK_MESH:-}" && -n "${COUCHLINK_MESH_IP:-}" ]]; then
    couchlink_apply_mesh_invite "$COUCHLINK_MESH" "$COUCHLINK_MESH_IP" "$port" "$platform"
    return $?
  fi

  local prefer="${COUCHLINK_MESH_PREFER:-headscale,tailscale,wireguard}"
  local part ip
  local -a _mesh_order=()
  IFS=',' read -ra _mesh_order <<<"$prefer"
  for part in "${_mesh_order[@]}"; do
    part="$(echo "$part" | tr -d '[:space:]')"
    case "$part" in
      headscale)
        # Only when enable-headscale (or operator) already marked the session.
        if [[ "${COUCHLINK_MESH:-}" == "headscale" && -n "${COUCHLINK_MESH_IP:-}" ]]; then
          couchlink_apply_mesh_invite headscale "$COUCHLINK_MESH_IP" "$port" "$platform"
          return 0
        fi
        # Live Tailscale client already joined this host's Headscale (has hs URL + 100.x).
        if [[ -n "${COUCHLINK_HS_URL:-}" ]]; then
          ip="$(couchlink_tailscale_ip 2>/dev/null || true)"
          if [[ -n "$ip" ]]; then
            couchlink_apply_mesh_invite headscale "$ip" "$port" "$platform"
            return 0
          fi
        fi
        ;;
      tailscale)
        ip="$(couchlink_tailscale_ip 2>/dev/null || true)"
        if [[ -n "$ip" ]]; then
          # Prefer headscale label when auth material is present for the invite URL.
          if [[ -n "${COUCHLINK_HS_URL:-}" && -n "${COUCHLINK_TS_AUTHKEY:-}" ]]; then
            couchlink_apply_mesh_invite headscale "$ip" "$port" "$platform"
          else
            couchlink_apply_mesh_invite tailscale "$ip" "$port" "$platform"
          fi
          return 0
        fi
        ;;
      wireguard)
        ip="$(couchlink_wireguard_ip 2>/dev/null || true)"
        if [[ -n "$ip" ]]; then
          couchlink_apply_mesh_invite wireguard "$ip" "$port" "$platform"
          return 0
        fi
        ;;
    esac
  done

  if [[ "${COUCHLINK_VERBOSE:-0}" == "1" ]]; then
    echo "==> no Headscale / Tailscale / WireGuard mesh up — falling back to UPnP / Cloudflare / IPv6"
  fi
  echo "    setup: ./scripts/enable-headscale.sh · ./scripts/setup-tailscale.sh · ./scripts/setup-wireguard.sh"
  echo "    docs:  docs/HEADSCALE.md · docs/MESH.md · docs/WIREGUARD.md"
  return 1
}
