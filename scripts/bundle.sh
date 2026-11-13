#!/usr/bin/env bash
# Builds target/Castoff.app: a release binary in a macOS app bundle, signed ad hoc (no Apple
# Developer account needed). `--install` also copies it to /Applications.
# CASTOFF_BIN=path/to/castoff bundles that binary (a universal one, in CI) instead of building.
set -euo pipefail
cd "$(dirname "$0")/.."

NAME=Castoff
ID=com.joncrangle.castoff
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/app/Cargo.toml | head -1)
APP=target/$NAME.app

BIN=${CASTOFF_BIN:-}
if [[ -z "$BIN" ]]; then
  cargo build --release -p castoff
  BIN=target/release/castoff
fi

# The icon: assets/icon.svg at every size macOS asks for.
ICONSET=target/icon.iconset
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
sips -s format png assets/icon.svg --out "$ICONSET/base.png" >/dev/null
for size in 16 32 128 256 512; do
  sips -z $size $size "$ICONSET/base.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) "$ICONSET/base.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
rm "$ICONSET/base.png"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/castoff"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/$NAME.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleExecutable</key><string>castoff</string>
  <key>CFBundleIconFile</key><string>$NAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSRemovableVolumesUsageDescription</key>
  <string>Castoff copies podcast episodes to your external drive.</string>
</dict>
</plist>
PLIST

codesign --force --sign - "$APP"
codesign --verify --strict "$APP"
echo "Built $APP ($(du -sh "$APP" | cut -f1))"

if [[ "${1:-}" == "--install" ]]; then
  rm -rf "/Applications/$NAME.app"
  cp -R "$APP" "/Applications/$NAME.app"
  echo "Installed /Applications/$NAME.app"
fi
