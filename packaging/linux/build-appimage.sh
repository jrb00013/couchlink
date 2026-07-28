#!/usr/bin/env bash
# Build a portable AppImage for the native Couchlink player (friend/client role).
# Requires: Rust toolchain. Optional: appimagetool (https://github.com/AppImage/AppImageKit)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

cargo build --release -p couchlink-client

ARCH="$(uname -m)"
APPDIR="$ROOT/build/CouchlinkPlayer-${ARCH}.AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications"

install -Dm755 target/release/couchlink-client "$APPDIR/usr/bin/couchlink-client"
install -Dm644 packaging/linux/couchlink-client.desktop "$APPDIR/couchlink-client.desktop"
install -Dm644 packaging/linux/couchlink-client.desktop "$APPDIR/usr/share/applications/couchlink-client.desktop"

cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
exec "$HERE/usr/bin/couchlink-client" "$@"
EOF
chmod +x "$APPDIR/AppRun"

ln -sf usr/bin/couchlink-client "$APPDIR/couchlink-client"

mkdir -p "$ROOT/build"
OUT="$ROOT/build/CouchlinkPlayer-${ARCH}.AppImage"
if command -v appimagetool >/dev/null; then
  ARCH="$ARCH" appimagetool "$APPDIR" "$OUT"
  echo "==> wrote $OUT"
else
  echo "==> AppDir ready: $APPDIR"
  echo "    Install appimagetool and re-run to produce $OUT"
fi

echo "Friend setup: save the host's join link to:"
echo "  \${XDG_CONFIG_HOME:-\$HOME/.config}/couchlink/config"
echo "  join_url=<paste full URL from host>"
