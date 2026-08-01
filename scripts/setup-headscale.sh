#!/usr/bin/env bash
# Download Headscale binary + write couchlink config under infra/headscale/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-platform.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/lib-headscale.sh"
HS_DIR="$ROOT/infra/headscale"
TOOLS="$ROOT/.tools"
BIN="$TOOLS/headscale"
DATA="$HS_DIR/data"
CFG="$HS_DIR/config.yaml"
EXAMPLE="$HS_DIR/config-example.yaml"

PLATFORM="$(couchlink_detect_platform)"
case "$PLATFORM" in
  linux|wsl) ;;
  *)
    echo "==> Headscale host binary is linux amd64 — on $PLATFORM use a Linux/WSL host" >&2
    echo "    (friend/client still joins via Tailscale client + hs=/tskey=)" >&2
    exit 0
    ;;
esac

mkdir -p "$TOOLS" "$DATA" "$HS_DIR"

if [[ ! -x "$BIN" ]]; then
  echo "==> downloading Headscale (linux amd64)…"
  ver="$(curl -fsSL https://api.github.com/repos/juanfont/headscale/releases/latest \
    | grep -oE '"tag_name": *"v[^"]+"' | head -1 | sed 's/.*"v/v/;s/"$//' || true)"
  ver="${ver:-v0.29.3}"
  ver_num="${ver#v}"
  url="https://github.com/juanfont/headscale/releases/download/${ver}/headscale_${ver_num}_linux_amd64"
  curl -fsSL -o "$BIN" --max-time 120 "$url"
  chmod +x "$BIN"
  echo "==> installed $BIN ($ver)"
else
  echo "==> Headscale binary present: $BIN"
fi

# Always refresh from example when missing, or when missing required DERP bootstrap URL.
need_rewrite=0
if [[ ! -f "$CFG" ]]; then
  need_rewrite=1
elif ! grep -q 'controlplane.tailscale.com/derpmap' "$CFG" 2>/dev/null; then
  echo "==> upgrading $CFG (add default DERP map — required for Headscale boot)"
  need_rewrite=1
fi

if [[ "$need_rewrite" == "1" ]]; then
  if [[ -f "$CFG" ]]; then
    cp -f "$CFG" "$CFG.bak.$(date +%s)" 2>/dev/null || true
  fi
  sed "s|REPLACE_DATA_DIR|${DATA}|g" "$EXAMPLE" >"$CFG"
  echo "==> wrote $CFG"
else
  # In-place upgrades for known conflicts
  if grep -q 'stun_listen_addr: "0.0.0.0:3479"' "$CFG" 2>/dev/null; then
    sed -i 's/stun_listen_addr: "0.0.0.0:3479"/stun_listen_addr: "0.0.0.0:34790"/' "$CFG"
    echo "==> upgraded STUN listen port to 34790 (avoid coturn clash)"
  fi
  echo "==> keep existing $CFG"
fi

echo "OK — next: ./scripts/enable-headscale.sh"
echo "    docs: docs/HEADSCALE.md"
