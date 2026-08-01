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
  # portable-ish: rewrite server_url line
  local tmp
  tmp="$(mktemp)"
  awk -v u="$url" -v d="$enable_derp" '
    /^server_url:/ { print "server_url: " u; next }
    /^    enabled:/ && derp_block { if (d=="1") print "    enabled: true"; else print "    enabled: false"; next }
    /^derp:/ { derp_block=1 }
    /^[^ ]/ && !/^derp:/ { derp_block=0 }
    { print }
  ' "$cfg" >"$tmp"
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
