#!/usr/bin/env bash
# Unit checks for cloudflared keepalive helpers (no live tunnel required).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-online-tunnel.sh"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
export COUCHLINK_JOIN_URL_FILE="$tmpdir/join.txt"
export COUCHLINK_CF_STATE_DIR="$tmpdir/cf"

printf '%s\n' \
  'https://old-name.trycloudflare.com/?s=abc&p=123&auto=1&ws=wss%3A%2F%2Fold-name.trycloudflare.com%2Fws&turn=turn%3A1.2.3.4%3A3478' \
  >"$COUCHLINK_JOIN_URL_FILE"

couchlink_cf_rewrite_join_hosts 'https://new-name.trycloudflare.com' >/dev/null
got="$(tr -d '\r\n' <"$COUCHLINK_JOIN_URL_FILE")"
[[ "$got" == *'https://new-name.trycloudflare.com/'* ]] || {
  echo "fail: https host not rewritten: $got" >&2
  exit 1
}
[[ "$got" == *'wss%3A%2F%2Fnew-name.trycloudflare.com'* ]] || {
  echo "fail: encoded wss host not rewritten: $got" >&2
  exit 1
}
[[ "$got" != *old-name* ]] || {
  echo "fail: old host still present: $got" >&2
  exit 1
}

if couchlink_cf_edge_ok 'https://this-host-definitely-does-not-exist-zz.trycloudflare.com'; then
  echo 'fail: edge_ok should fail on NXDOMAIN' >&2
  exit 1
fi

echo 'ok cloudflared-keepalive helpers'
