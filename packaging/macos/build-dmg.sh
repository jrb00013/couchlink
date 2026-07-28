#!/usr/bin/env bash
# Build Couchlink Player.dmg for macOS (drag-to-Applications installer experience).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

"$ROOT/packaging/macos/build-app-bundle.sh"

APP="$ROOT/build/Couchlink Player.app"
DMG="$ROOT/build/CouchlinkPlayer-mac.dmg"
STAGE="$ROOT/build/dmg-stage"
rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

hdiutil create -volname "Couchlink Player" -srcfolder "$STAGE" -ov -format UDZO -imagekey zlib-level=9 "$DMG"
rm -rf "$STAGE"

echo "==> wrote $DMG"
echo "    Friend: open DMG → drag Couchlink Player to Applications → set join_url in config (see docs/DESKTOP_CLIENT.md)"
