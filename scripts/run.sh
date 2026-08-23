#!/usr/bin/env bash
# One command to run couchlink: ./scripts/run.sh [host|client] [--local|--online] [--verbose] [--unblock-firewall]
# Auto-detects platform (Linux / WSL / macOS), starts signaling + TURN + host
# (or just the client) as background child processes of this one script, and
# tears them all down together on Ctrl-C. No separate terminals needed.
#
# Reachability:
#   host  --local   (default) same Wi‑Fi / LAN — join URL uses LAN IP, no UPnP/TURN
#   host  --online  internet — PRIME: Headscale / Tailscale / WireGuard mesh if up;
#                   else public IP + TURN + UPnP; then Cloudflare HTTPS + IPv6 / bore fallback
#   client --online requires host TURN (join URL or COUCHLINK_TURN_*) unless on mesh;
#                   WSL auto ICE IPs; Headscale invites auto-join via hs=+tskey=
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-upnp.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-online-tunnel.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-mesh.sh"

usage() {
  cat <<EOF
usage: $0 [host|client] [--local|--online] [--verbose] [--unblock-firewall] [--force-cloudflare]

  host    start signaling + (optional TURN) + couchlink-host
  client  start couchlink-client (friend/player)

  --local   LAN only (default). Host: LAN join URL. Client: TURN optional.
  --online  Internet. Host prefers Headscale / Tailscale / WireGuard (PRIME mesh)
            when up; else public IP + TURN + UPnP; on WSL also firewall + WSL
            portproxy; then Cloudflare HTTPS + IPv6 / bore if the router blocks UPnP.
            Client: prompts for the host join URL if unset; auto-joins Headscale
            when the invite has hs= + tskey=.
  --force-cloudflare
            Host: skip mesh + UPnP entirely and always bring up a cloudflared
            HTTPS invite (https://*.trycloudflare.com). Use when the mesh or a
            direct public IP can't reach the friend. Requires --online.
  --verbose
            Chatty bring-up logs, QR code, TRACE-level-ish Rust logs (RUST_LOG).
            Default is quiet — join URL is still printed.
  --unblock-firewall
            Client: open local OS firewall for mesh/TURN (Windows UAC once).

  Mesh: ./scripts/enable-headscale.sh · ./scripts/setup-wireguard.sh
  (optional Tailscale Inc cloud: ./scripts/setup-tailscale.sh — not auto-run)
  Docs: docs/HEADSCALE.md · docs/MESH.md · docs/WIREGUARD.md

Platform is auto-detected (linux / wsl / macos).
EOF
}

ROLE="host"
MODE="local"
UNBLOCK_FIREWALL=0
FORCE_CLOUDFLARE=0
COUCHLINK_VERBOSE="${COUCHLINK_VERBOSE:-0}"
for arg in "$@"; do
  case "$arg" in
    host|client) ROLE="$arg" ;;
    --local) MODE="local" ;;
    --online) MODE="online" ;;
    --force-cloudflare) FORCE_CLOUDFLARE=1 ;;
    --verbose|-v) COUCHLINK_VERBOSE=1 ;;
    --unblock-firewall) UNBLOCK_FIREWALL=1 ;;
    -h|--help) usage; exit 0 ;;
    *)
      echo "unknown argument: $arg" >&2
      usage >&2
      exit 1
      ;;
  esac
done
export COUCHLINK_VERBOSE

if [[ "$FORCE_CLOUDFLARE" == "1" && "$MODE" != "online" ]]; then
  echo "error: --force-cloudflare requires --online" >&2
  usage >&2
  exit 1
fi
if [[ "$FORCE_CLOUDFLARE" == "1" ]]; then
  couchlink_say "==> --force-cloudflare: skipping mesh + UPnP, forcing cloudflared HTTPS invite"
  export COUCHLINK_FORCE_CLOUDFLARE=1 COUCHLINK_SKIP_MESH=1 COUCHLINK_SKIP_UPNP=1
fi

# Quiet Rust crates unless the user opted in or already set RUST_LOG.
if ! couchlink_verbose && [[ -z "${RUST_LOG:-}" ]]; then
  export RUST_LOG="warn,couchlink_host=warn,couchlink_signaling=warn,webrtc=error,webrtc_ice=error,hyper=error,tower_http=error"
fi

PLATFORM="$(couchlink_detect_platform)"
couchlink_say "==> platform: $PLATFORM · role: $ROLE · mode: $MODE$(couchlink_verbose && echo ' · verbose' || true)"

# Put Homebrew / cargo on PATH for macOS (system bash often lacks them).
export PATH="$(couchlink_tool_path "${HOME:-}")${PATH:+:$PATH}"

if [[ "$ROLE" == "host" && "$PLATFORM" == "macos" ]]; then
  echo "note: macOS host is video-only — no virtual DualSense (uinput is Linux/WSL)."
  echo "      Friend pad input will not inject; use Linux/WSL host for full co-play."
fi

[[ -f .env.couchlink ]] || cp .env.example .env.couchlink
# Preserve capture overrides from the parent shell across .env.couchlink — otherwise
# a pinned COUCHLINK_CAPTURE_WINDOW=PCSX2 silently forces window mode and the
# picker never appears (and win-capture waits forever for a closed emulator).
_HAS_CAPTURE_SOURCE=0
_HAS_CAPTURE_WINDOW=0
[[ -v COUCHLINK_CAPTURE_SOURCE ]] && _HAS_CAPTURE_SOURCE=1
[[ -v COUCHLINK_CAPTURE_WINDOW ]] && _HAS_CAPTURE_WINDOW=1
_KEEP_CAPTURE_SOURCE="${COUCHLINK_CAPTURE_SOURCE:-}"
_KEEP_CAPTURE_WINDOW="${COUCHLINK_CAPTURE_WINDOW:-}"
# shellcheck disable=SC1091
# set -a: .env.couchlink assigns without `export`, and these settings have to
# reach child scripts (ensure-win-capture.sh reads COUCHLINK_PRESET itself).
set -a
source .env.couchlink
set +a
if [[ "$_HAS_CAPTURE_SOURCE" == "1" ]]; then
  export COUCHLINK_CAPTURE_SOURCE="$_KEEP_CAPTURE_SOURCE"
fi
if [[ "$_HAS_CAPTURE_WINDOW" == "1" ]]; then
  export COUCHLINK_CAPTURE_WINDOW="$_KEEP_CAPTURE_WINDOW"
fi

# Drop a persisted ICE address this machine no longer holds.
#
# COUCHLINK_ICE_IPS becomes webrtc's nat_1to1_ips, which *rewrites* every host
# candidate to that address. A stale value therefore does not merely add a dead
# candidate — it replaces the good ones, so every host candidate the player
# receives points somewhere unreachable. Switching WSL from NAT to mirrored
# networking retires the old eth0 IP and leaves exactly this trap behind: ICE
# reaches `connected` on some other pair, then drops, and the viewer sits on
# "video track" with no frames.
if [[ -n "${COUCHLINK_ICE_IPS:-}" && "${COUCHLINK_ICE_IPS}" != */* ]]; then
  _stale=""
  for _ip in ${COUCHLINK_ICE_IPS//,/ }; do
    ip -o addr show 2>/dev/null | grep -qF " ${_ip}/" || _stale="${_stale} ${_ip}"
  done
  if [[ -n "${_stale// /}" ]]; then
    couchlink_say "==> dropping ICE address(es) this machine no longer holds:${_stale}"
    unset COUCHLINK_ICE_IPS
  fi
  unset _stale _ip
fi

if [[ "$ROLE" == "host" && ( -z "${COUCHLINK_SESSION_ID:-}" || -z "${COUCHLINK_PIN:-}" ) ]]; then
  couchlink_say "==> no session set — generating one"
  eval "$(./scripts/gen_session.sh)"
  {
    echo "COUCHLINK_SESSION_ID=$COUCHLINK_SESSION_ID"
    echo "COUCHLINK_PIN=$COUCHLINK_PIN"
  } >> .env.couchlink
fi

# Reachability overrides — must win over whatever is in .env.couchlink for this run.
export COUCHLINK_MODE="$MODE"
PORT="${COUCHLINK_BIND##*:}"
PORT="${PORT:-8443}"
COUCHLINK_USING_MESH=0

# On --online (WSL/Windows): Private profile + discovery + firewall + WSL
# portproxy + NATUPnP maps. Prefer Couchlink Helper service (no UAC); else
# Scheduled Task; else COUCHLINK_ALLOW_UAC=1.
couchlink_try_upnp_online() {
  [[ "${COUCHLINK_SKIP_UPNP_PREP:-}" == "1" ]] && return 0
  [[ "${COUCHLINK_FORCE_CLOUDFLARE:-0}" == "1" ]] && return 1
  local ok=0
  if [[ "$PLATFORM" == "wsl" || "$PLATFORM" == "windows" ]] && command -v powershell.exe >/dev/null 2>&1; then
    couchlink_vlog "==> --online: Windows prep (Helper / task / firewall + WSL portproxy + UPnP)"
    set +e
    if couchlink_verbose; then
      bash "$ROOT/scripts/enable-upnp.sh"
    else
      bash "$ROOT/scripts/enable-upnp.sh" >/dev/null 2>&1
    fi
    local ec=$?
    set -e
    [[ "$ec" -eq 0 ]] && ok=1
    # Retry map-only COM helper if prep left IGD visible but map exit was 2.
    if [[ "$ok" != "1" ]]; then
      local bridge_w
      bridge_w="$(wslpath -w "$ROOT/scripts/windows/open-ports-upnp.ps1" 2>/dev/null || true)"
      if [[ -n "${bridge_w:-}" ]]; then
        if couchlink_verbose; then
          powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$bridge_w" && ok=1 || true
        else
          powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$bridge_w" >/dev/null 2>&1 && ok=1 || true
        fi
      fi
    fi
  fi
  if upnp_open "$PORT" tcp "signaling"; then
    ok=1
  fi
  upnp_open 3478 udp "turn" || true
  upnp_open 3478 tcp "turn" || true
  return $((1 - ok))
}

# When the router won't forward IPv4:
#   1) cloudflared HTTPS signaling (browser WebCodecs needs a secure context)
#   2) IPv6 TURN/invite when Windows has a global IPv6 (UDP/TCP without router NAT)
#   3) bore signaling-only last resort — never put TURN on bore
couchlink_apply_online_fallback() {
  local public_ip="$1"
  local v6=""
  v6="$(couchlink_read_public_ipv6 2>/dev/null || true)"

  local used_cf=0
  # COUCHLINK_NO_CLOUDFLARE=1 keeps every byte between the two of you: signaling
  # falls through to the direct IPv6 URL below instead of a third party.
  #
  # The cost is a secure context. Browsers gate WebCodecs behind HTTPS, and a
  # bare ws:// invite is not one — a browser friend silently drops to the RTP
  # path. Native clients and mesh (Headscale/WireGuard) URLs are unaffected,
  # which is why this is opt-in rather than the default.
  if [[ "${COUCHLINK_NO_CLOUDFLARE:-0}" == "1" ]]; then
    couchlink_say "==> cloudflared disabled (COUCHLINK_NO_CLOUDFLARE=1) — direct signaling only"
  elif couchlink_start_cloudflared "$ROOT" "$PORT"; then
    used_cf=1
    # Host still dials loopback; friends get https://*.trycloudflare.com
    export COUCHLINK_INVITE_SIGNALING="${COUCHLINK_CF_URL/https:/wss:}/ws"
    couchlink_say "==> HTTPS invite via cloudflared (WebCodecs unlocked in browser)"
  fi

  if [[ -n "$v6" ]]; then
    local v6br
    v6br="$(couchlink_bracket_host "$v6")"
    # Signaling over the Windows IPv6 is fine — it is TCP, and portproxy
    # forwards v6->WSL. TURN is UDP and cannot be forwarded that way.
    if [[ "$used_cf" != "1" ]]; then
      export COUCHLINK_INVITE_SIGNALING="ws://${v6br}:${PORT}/ws"
    fi
    if couchlink_owns_ipv6 "$v6"; then
      export COUCHLINK_TURN_URL="turn:${v6br}:3478"
      export COUCHLINK_TURN_EXTERNAL_IP="$v6"
      couchlink_say "==> TURN on public IPv6 ${v6} (no IPv4 port forward needed)"
      return 0
    fi
    couchlink_say "==> WARN: public IPv6 ${v6} belongs to Windows, not this WSL instance —"
    couchlink_say "         not advertising TURN there (UDP cannot be portproxied to WSL)."
    couchlink_say "         Friends behind a strict NAT will fail ICE until one of:"
    couchlink_say "           * forward UDP+TCP 3478 to this PC, or"
    couchlink_say "           * WSL mirrored networking (.wslconfig networkingMode=mirrored)"
  fi

  # No IPv6 — keep TURN on WAN IPv4 (needs UPnP/forward to actually work).
  export COUCHLINK_TURN_URL="turn:${public_ip}:3478"
  export COUCHLINK_TURN_EXTERNAL_IP="$public_ip"

  if [[ "$used_cf" == "1" ]]; then
    couchlink_say "==> WARN: no public IPv6 — TURN still ${public_ip}:3478 (forward UDP/TCP 3478 if ICE fails)"
    return 0
  fi

  if couchlink_start_bore_signaling "$ROOT" "$PORT"; then
    export COUCHLINK_INVITE_SIGNALING="ws://bore.pub:${COUCHLINK_BORE_SIG_PORT}/ws"
    couchlink_say "==> bore signaling only; TURN remains ${public_ip}:3478"
    couchlink_vlog "    browser WebCodecs needs https — prefer cloudflared; native client is fine on http"
    return 0
  fi

  couchlink_say "==> UPnP incomplete — keeping IPv4 invite ${public_ip} (may need manual forward)"
  couchlink_vlog "    forward TCP ${PORT} + UDP/TCP 3478 to this PC, or re-run ./scripts/enable-upnp.sh"
  return 1
}

if [[ "$ROLE" == "host" ]]; then
  if [[ "$MODE" == "local" ]]; then
    LAN_IP="$(upnp_local_ip)"
    LAN_IP="${LAN_IP:-127.0.0.1}"
    export COUCHLINK_SIGNALING="ws://${LAN_IP}:${PORT}/ws"
    # Don't advertise a public TURN relay on a LAN session.
    unset COUCHLINK_TURN_URL || true
    couchlink_say "==> local mode — join URL will use LAN IP ${LAN_IP} (no UPnP / TURN)"
  else
    # PRIME: Headscale first (self-hosted; paste-link, no Tailscale Inc for friends).
    if [[ "${COUCHLINK_SKIP_MESH:-0}" == "1" ]]; then
      unset COUCHLINK_MESH COUCHLINK_MESH_IP COUCHLINK_HS_URL COUCHLINK_TS_AUTHKEY \
        COUCHLINK_MESH_NEED_TURN COUCHLINK_HS_SOCKET COUCHLINK_HS_LOCAL_LOGIN || true
    elif [[ "${COUCHLINK_SKIP_HEADSCALE:-0}" != "1" ]]; then
      if [[ -z "${COUCHLINK_MESH_IP:-}" || "${COUCHLINK_MESH:-}" != "headscale" ]]; then
        couchlink_say "==> bringing up Headscale mesh (PRIME)…"
        if couchlink_run_quiet "$ROOT/.run/headscale-bringup.log" bash "$ROOT/scripts/enable-headscale.sh"; then
          # shellcheck disable=SC1091
          source "$ROOT/infra/headscale/data/mesh.env"
          # Keep Headscale's cloudflared tunnel alive with this run.
          if [[ -n "${COUCHLINK_TUNNEL_PIDS[*]:-}" ]]; then
            :
          fi
          couchlink_say "==> Headscale ready (mesh ${COUCHLINK_MESH_IP:-?})"
        else
          couchlink_say "==> Headscale bring-up skipped/failed — trying Tailscale / WireGuard / public"
        fi
      fi
    fi
    # Tailscale cloud (if already up) or WireGuard fallback when Headscale didn't apply.
    if [[ "${COUCHLINK_SKIP_MESH:-0}" != "1" ]]; then
      if [[ -z "${COUCHLINK_MESH_IP:-}" ]]; then
        if couchlink_tailscale_ip >/dev/null 2>&1; then
          :
        elif [[ "${COUCHLINK_AUTO_WIREGUARD:-1}" != "0" ]]; then
          if [[ -f "$ROOT/infra/wireguard/wg0-host.conf" ]] || [[ "${COUCHLINK_ENSURE_WIREGUARD:-0}" == "1" ]]; then
            couchlink_vlog "==> no Headscale/Tailscale mesh — ensuring WireGuard tunnel"
            bash "$ROOT/scripts/enable-wireguard.sh" || \
              couchlink_say "==> WireGuard bring-up failed — will try public fallback"
          fi
        fi
      fi
    fi
    if [[ "${COUCHLINK_SKIP_MESH:-0}" != "1" ]] && couchlink_try_mesh_online "$PORT" "$PLATFORM"; then
      COUCHLINK_USING_MESH=1
      # WSL: friends hit Windows mesh IP (Tailscale 100.x / WG) — need portproxy → WSL.
      if [[ "$PLATFORM" == "wsl" && "${COUCHLINK_SKIP_UPNP_PREP:-}" != "1" ]]; then
        couchlink_vlog "==> WSL mesh: Windows firewall + portproxy for ${COUCHLINK_MESH_IP:-mesh}"
        if couchlink_verbose; then
          bash "$ROOT/scripts/enable-upnp.sh" --skip-map >/dev/null 2>&1 \
            || couchlink_say "==> portproxy prep skipped/failed — if join fails, run ./scripts/enable-upnp.sh --skip-map"
        else
          bash "$ROOT/scripts/enable-upnp.sh" --skip-map >/dev/null 2>&1 || true
        fi
      fi
    else
      PUBLIC_IP="${COUCHLINK_PUBLIC_IP:-}"
      if [[ -z "$PUBLIC_IP" ]]; then
        PUBLIC_IP="$(curl -fsS --max-time 5 ifconfig.me 2>/dev/null || true)"
      fi
      if [[ -z "$PUBLIC_IP" ]]; then
        echo "online mode needs a public IP (curl ifconfig.me failed) or an up mesh." >&2
        echo "Set COUCHLINK_PUBLIC_IP, or: ./scripts/enable-headscale.sh / setup-tailscale / setup-wireguard" >&2
        exit 1
      fi
      export COUCHLINK_PUBLIC_IP="$PUBLIC_IP"
      # Host must dial signaling on loopback/LAN — WSL/NAT often cannot hairpin
      # back to the public IP. Friends still get the public invite URL below.
      export COUCHLINK_SIGNALING="ws://127.0.0.1:${PORT}/ws"
      export COUCHLINK_INVITE_SIGNALING="ws://${PUBLIC_IP}:${PORT}/ws"
      export COUCHLINK_TURN_URL="turn:${PUBLIC_IP}:3478"
      couchlink_say "==> online mode — public IP ${PUBLIC_IP} (TURN + UPnP; host dials 127.0.0.1)"
      if couchlink_try_upnp_online; then
        couchlink_vlog "==> UPnP OK — ports should be reachable at ${PUBLIC_IP}"
        export COUCHLINK_SKIP_UPNP=1
      else
        couchlink_apply_online_fallback "$PUBLIC_IP" || true
      fi
    fi
  fi
elif [[ "$ROLE" == "client" ]]; then
  if [[ "$UNBLOCK_FIREWALL" == "1" ]]; then
    bash "$ROOT/scripts/unblock-firewall.sh" || \
      couchlink_say "==> unblock-firewall failed (continuing)"
  fi
  if [[ "$MODE" == "online" ]]; then
    # Prompt early so Headscale auto-join can run before the UI starts.
    if [[ -z "${COUCHLINK_JOIN_URL:-}" ]]; then
      if [[ -t 0 ]]; then
        echo -n "Paste host join URL (or Enter to type later in the player): "
        read -r _join || true
        if [[ -n "${_join:-}" ]]; then
          export COUCHLINK_JOIN_URL="$_join"
        fi
      fi
    fi
    if couchlink_try_client_headscale_join "$ROOT"; then
      couchlink_say "==> Headscale join OK — starting player"
    else
      # Opt-in only: Tailscale Inc cloud. Default stays quiet (no Windows popup).
      if [[ "${COUCHLINK_ENSURE_TAILSCALE_CLOUD:-0}" == "1" ]]; then
        couchlink_ensure_client_tailscale "$ROOT" || true
      fi
    fi
    if [[ -n "${COUCHLINK_JOIN_URL:-}" ]]; then
      couchlink_vlog "==> online client — join URL set (TURN / mesh from invite)"
    elif [[ -n "${COUCHLINK_TURN_URL:-}" && -n "${COUCHLINK_TURN_USER:-}" && -n "${COUCHLINK_TURN_PASS:-}" ]]; then
      couchlink_vlog "==> online client — TURN credentials from env"
    else
      couchlink_say "==> online client — paste the host join URL when prompted"
    fi
  else
    couchlink_vlog "==> local client — paste join URL if credentials are missing"
  fi
fi

PIDS=()
cleanup() {
  couchlink_say "==> shutting down"
  for pid in "${PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  for pid in "${COUCHLINK_TUNNEL_PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  for pid in "${COUCHLINK_BORE_PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  if [[ "$PLATFORM" == "wsl" && "$ROLE" == "host" ]]; then
    # Host started win-capture via powershell; stop it with the session.
    case "${COUCHLINK_WINDOWS_CAPTURE:-auto}" in
      0|false|local|off) ;;
      *)
        if command -v taskkill.exe >/dev/null 2>&1; then
          taskkill.exe /IM couchlink-win-capture.exe /F >/dev/null 2>&1 || true
        fi
        ;;
    esac
  fi
  wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

if [[ "$ROLE" == "host" ]]; then
  ./scripts/start-signaling.sh &
  PIDS+=($!)
  # Wait until signaling actually accepts TCP — a fixed sleep races the host's
  # first dial (connection refused) and leaves friends on "Waiting for host"
  # until a later retry catches up. Prefer ready over hopeful.
  _sig_port="${COUCHLINK_BIND:-0.0.0.0:8443}"
  _sig_port="${_sig_port##*:}"
  for _ in $(seq 1 50); do
    if timeout 0.2 bash -c "echo >/dev/tcp/127.0.0.1/${_sig_port}" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  # Mesh on native Linux/macOS: skip TURN. WSL mesh: keep TURN (UDP via portproxy).
  if [[ "$MODE" == "online" ]]; then
    if [[ "$COUCHLINK_USING_MESH" != "1" || "${COUCHLINK_MESH_NEED_TURN:-0}" == "1" ]]; then
      ./scripts/start-turn.sh &
      PIDS+=($!)
      sleep 1
    fi
  fi
  ./scripts/start-host.sh &
  PIDS+=($!)
else
  ./scripts/start-client.sh &
  PIDS+=($!)
fi

# wait -n needs bash ≥ 4.3; macOS /bin/bash is still 3.2.
if [[ "${BASH_VERSINFO[0]}" -gt 4 ]] \
  || { [[ "${BASH_VERSINFO[0]}" -eq 4 ]] && [[ "${BASH_VERSINFO[1]}" -ge 3 ]]; }; then
  wait -n "${PIDS[@]}"
else
  while true; do
    for pid in "${PIDS[@]}"; do
      if ! kill -0 "$pid" 2>/dev/null; then
        wait "$pid"
        exit $?
      fi
    done
    sleep 0.5
  done
fi
