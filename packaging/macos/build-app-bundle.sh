#!/usr/bin/env bash
# Build Couchlink Player.app for macOS (friend/client role).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

cargo build --release -p couchlink-client

APP="$ROOT/build/Couchlink Player.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

install -m755 target/release/couchlink-client "$APP/Contents/MacOS/couchlink-client"

cat > "$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>couchlink-client</string>
  <key>CFBundleIdentifier</key>
  <string>com.couchlink.player</string>
  <key>CFBundleName</key>
  <string>Couchlink Player</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF

echo "==> wrote $APP"
echo "Friend config: ~/Library/Application Support/Couchlink/config"
echo "  join_url=<host join link>"
echo ""
echo "First launch: right-click → Open (unsigned dev build), or codesign for distribution."
