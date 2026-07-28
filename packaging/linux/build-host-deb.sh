#!/usr/bin/env bash
# Build couchlink-host_*.deb — installable host for non-technical Linux users.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

cargo build --release -p couchlink-host -p couchlink-signaling
cc -O2 -Wall -Wextra -o native/uinput-helper/couchlink-uinput-helper \
  native/uinput-helper/couchlink-uinput-helper.c

VER=0.1.1
PKG="$ROOT/build/deb/couchlink-host_${VER}_amd64"
rm -rf "$PKG"
install -dm755 "$PKG/DEBIAN" "$PKG/usr/bin" "$PKG/usr/share/applications" \
  "$PKG/etc/udev/rules.d" "$PKG/usr/share/couchlink"

install -m755 target/release/couchlink-host "$PKG/usr/bin/couchlink-host"
install -m755 target/release/couchlink-signaling "$PKG/usr/bin/couchlink-signaling"
install -m755 native/uinput-helper/couchlink-uinput-helper "$PKG/usr/bin/couchlink-uinput-helper"
install -m755 packaging/linux/couchlink-run-host.sh "$PKG/usr/bin/couchlink-run-host"
install -m644 packaging/linux/couchlink-host.desktop "$PKG/usr/share/applications/"
install -m644 packaging/linux/couchlink-host-setup.desktop "$PKG/usr/share/applications/"
install -m644 packaging/linux/host-deb/DEBIAN/control "$PKG/DEBIAN/control"
install -m755 packaging/linux/host-deb/DEBIAN/postinst "$PKG/DEBIAN/postinst"
printf '%s\n' 'KERNEL=="uinput", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"' \
  >"$PKG/etc/udev/rules.d/99-couchlink-uinput.rules"

if [[ -d web/dist ]]; then
  cp -a web/dist "$PKG/usr/share/couchlink/web"
fi

OUT="$ROOT/build/couchlink-host_${VER}_amd64.deb"
mkdir -p "$ROOT/build"
dpkg-deb --build "$PKG" "$OUT"
echo "==> wrote $OUT"
