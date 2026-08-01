#!/usr/bin/env bash
# Sourced helpers for Headscale bring-up (control plane + embedded DERP + keys).

couchlink_headscale_bin() {
  local root="${1:-}"
  local bin="${root}/.tools/headscale"
  [[ -x "$bin" ]] && { printf '%s' "$bin"; return 0; }
  command -v headscale 2>/dev/null || return 1
}

couchlink_headscale_running() {
  local sock="${1}/infra/headscale/data/headscale.sock"
  [[ -S "$sock" ]] || return 1
  return 0
}

# Rewrite server_url + optionally enable embedded DERP in config.yaml
couchlink_headscale_set_server_url() {
  local cfg="$1"
  local url="$2"
  local enable_derp="${3:-0}"
  [[ -f "$cfg" ]] || return 1
  local tmp
  tmp="$(mktemp)"
  awk -v u="$url" -v d="$enable_derp" '
    /^server_url:/ { print "server_url: " u; next }
    /^    enabled:/ && derp_server {
      if (d=="1") print "    enabled: true";
      else print "    enabled: false";
      next
    }
    /^  server:/ && derp_block { derp_server=1 }
    /^derp:/ { derp_block=1; derp_server=0 }
    /^[^ ]/ && !/^derp:/ { derp_block=0; derp_server=0 }
    /^  urls:/ && derp_block && !derp_server { in_urls=1; print; next }
    in_urls && /^  [^ ]/ { in_urls=0 }
    in_urls && /^    -/ { next }
    { print }
  ' "$cfg" >"$tmp"
  # Ensure at least the default DERP map URL exists when embedded is off.
  if [[ "$enable_derp" != "1" ]] && ! grep -q 'controlplane.tailscale.com/derpmap' "$tmp"; then
    awk '
      /^  urls:/ && !done {
        print
        print "    - https://controlplane.tailscale.com/derpmap/default"
        done=1
        next
      }
      { print }
    ' "$tmp" >"${tmp}.2"
    mv "${tmp}.2" "$tmp"
  fi
  mv "$tmp" "$cfg"
}

couchlink_headscale_cli() {
  local root="$1"
  shift
  local bin cfg
  bin="$(couchlink_headscale_bin "$root")" || return 1
  cfg="$root/infra/headscale/config.yaml"
  "$bin" --config "$cfg" "$@"
}

# Print numeric user id for name (Headscale v0.29+ preauthkeys need -u uint).
couchlink_headscale_user_id() {
  local root="$1"
  local name="$2"
  couchlink_headscale_cli "$root" users list -o json 2>/dev/null | python3 -c '
import json, sys
name = sys.argv[1]
raw = sys.stdin.read().strip()
if not raw:
    sys.exit(1)
data = json.loads(raw)
if isinstance(data, dict):
    data = data.get("users") or data.get("Users") or []
for u in data:
    if not isinstance(u, dict):
        continue
    n = u.get("name") or u.get("Name") or ""
    if n == name:
        uid = u.get("id") if u.get("id") is not None else u.get("ID")
        if uid is not None:
            print(uid)
            raise SystemExit(0)
sys.exit(1)
' "$name"
}

# Ensure user exists; print numeric id.
couchlink_headscale_ensure_user() {
  local root="$1"
  local name="$2"
  local uid=""
  uid="$(couchlink_headscale_user_id "$root" "$name" 2>/dev/null || true)"
  if [[ -z "$uid" ]]; then
    couchlink_headscale_cli "$root" users create "$name" >/dev/null 2>&1 || true
    uid="$(couchlink_headscale_user_id "$root" "$name" 2>/dev/null || true)"
  fi
  [[ -n "$uid" ]] || return 1
  printf '%s' "$uid"
}

# Mint a reusable ephemeral preauth key for user id; print the key.
couchlink_headscale_mint_preauth() {
  local root="$1"
  local uid="$2"
  local out
  out="$(couchlink_headscale_cli "$root" preauthkeys create \
    -u "$uid" --reusable --ephemeral -e 168h -o json 2>/dev/null || true)"
  if [[ -z "$out" ]]; then
    out="$(couchlink_headscale_cli "$root" preauthkeys create \
      -u "$uid" --reusable --ephemeral --expiration 168h 2>/dev/null || true)"
  fi
  python3 -c '
import json, re, sys
raw = sys.stdin.read().strip()
if not raw:
    sys.exit(1)
try:
    d = json.loads(raw)
    key = d.get("key") or d.get("Key") or ""
    if key:
        print(key)
        raise SystemExit(0)
except Exception:
    pass
m = re.search(r"(?:hskey|tskey)-[A-Za-z0-9_-]+", raw)
if m:
    print(m.group(0))
    raise SystemExit(0)
sys.exit(1)
' <<<"$out"
}

# Userspace tailscaled for Headscale (no sudo / no Windows Tailscale popup).
# Prints socket path on stdout.
couchlink_headscale_userspace_start() {
  local root="$1"
  local name="${2:-host}"
  local dir="$root/infra/headscale/data/us-$name"
  local sock="$dir/tailscaled.sock"
  local pidf="$dir/tailscaled.pid"
  local logf="$dir/tailscaled.log"
  local tsbin
  tsbin="$(command -v tailscaled 2>/dev/null || true)"
  [[ -n "$tsbin" ]] || tsbin="/usr/sbin/tailscaled"
  [[ -x "$tsbin" ]] || return 1
  mkdir -p "$dir"
  if [[ -f "$pidf" ]] && kill -0 "$(cat "$pidf")" 2>/dev/null; then
    [[ -S "$sock" ]] && { printf '%s' "$sock"; return 0; }
  fi
  if [[ -f "$pidf" ]]; then
    kill "$(cat "$pidf")" 2>/dev/null || true
    rm -f "$pidf"
  fi
  rm -f "$sock"
  "$tsbin" --tun=userspace-networking --statedir="$dir" --socket="$sock" >"$logf" 2>&1 &
  echo $! >"$pidf"
  local i
  for i in $(seq 1 30); do
    [[ -S "$sock" ]] && { printf '%s' "$sock"; return 0; }
    sleep 0.2
  done
  return 1
}

# Join Headscale via userspace client. Args: root name login_url auth_key
couchlink_headscale_userspace_up() {
  local root="$1" name="$2" login="$3" key="$4"
  local sock ts
  sock="$(couchlink_headscale_userspace_start "$root" "$name")" || return 1
  ts="$(command -v tailscale)" || return 1
  "$ts" --socket="$sock" up --login-server="$login" --auth-key="$key" \
    --accept-dns=false --accept-routes=false --hostname="couchlink-${name}" \
    || return 1
  printf '%s' "$sock"
}

couchlink_headscale_userspace_ip() {
  local sock="$1"
  local ts
  ts="$(command -v tailscale)" || return 1
  "$ts" --socket="$sock" ip -4 2>/dev/null | head -1 | tr -d ' \r\n'
}
