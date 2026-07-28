#!/usr/bin/env bash
# Build couchlink-player_0.1.1_amd64.deb (Ubuntu/Debian friends).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

cargo build --release -p couchlink-client

PKG="$ROOT/build/deb/couchlink-player_0.1.1_amd64"
rm -rf "$PKG"
install -dm755 "$PKG/DEBIAN"
install -dm755 "$PKG/usr/bin"
install -dm755 "$PKG/usr/share/applications"
install -dm755 "$PKG/etc/xdg/couchlink"

install -m755 target/release/couchlink-client "$PKG/usr/bin/couchlink-client"
install -m644 packaging/linux/couchlink-client.desktop "$PKG/usr/share/applications/couchlink-client.desktop"
install -m644 packaging/config.example "$PKG/etc/xdg/couchlink/config.example"
install -m644 packaging/linux/deb/DEBIAN/control "$PKG/DEBIAN/control"

OUT="$ROOT/build/couchlink-player_0.1.1_amd64.deb"
dpkg-deb --build "$PKG" "$OUT"
echo "==> wrote $OUT"
echo "    Friend: sudo dpkg -i $(basename "$OUT")"
echo "    Config: ~/.config/couchlink/config  (join_url=...)"
