#!/usr/bin/env bash
# Builds postrust.app and a .dmg from an already-built binary.
# Usage: scripts/make-macos-app.sh <binary> <version> <output-dir> <dmg-name>
set -euo pipefail

BIN="$1"
VERSION="$2"
OUT="$3"
DMG="$4"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# macOS expects period-separated integers, so drop any -beta.1 / +build suffix.
# Single source for the bundle id: [package.metadata.bundle] in Cargo.toml.
IDENTIFIER="$(grep -m1 '^identifier' "$ROOT/Cargo.toml" | sed -E 's/^identifier *= *"([^"]+)".*/\1/')"
[ -n "$IDENTIFIER" ] || { echo "bundle identifier not found in Cargo.toml" >&2; exit 1; }
PLIST_VERSION="${VERSION%%[-+]*}"

APP="$OUT/postrust.app"
rm -rf "${APP:?}" "${OUT:?}/dmg-root" "${OUT:?}/${DMG:?}"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/postrust"
chmod +x "$APP/Contents/MacOS/postrust"
cp "$ROOT/assets/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>postrust</string>
  <key>CFBundleDisplayName</key><string>postrust</string>
  <key>CFBundleIdentifier</key><string>${IDENTIFIER}</string>
  <key>CFBundleExecutable</key><string>postrust</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${PLIST_VERSION}</string>
  <key>CFBundleVersion</key><string>${PLIST_VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc signature: Apple Silicon refuses to run unsigned bundles. This is not notarization,
# so users still see the "unidentified developer" prompt on first launch.
codesign --force --deep --sign - "$APP"

mkdir "$OUT/dmg-root"
cp -R "$APP" "$OUT/dmg-root/"
ln -s /Applications "$OUT/dmg-root/Applications"
# hdiutil is occasionally flaky ("Resource busy") on CI runners, so retry a few times.
for attempt in 1 2 3 4; do
  if hdiutil create -volname "postrust" -srcfolder "$OUT/dmg-root" -ov -format UDZO "$OUT/$DMG" >/dev/null; then
    break
  fi
  [ "$attempt" -lt 4 ] || { echo "hdiutil failed after $attempt attempts" >&2; exit 1; }
  echo "hdiutil failed (attempt $attempt), retrying..." >&2
  sleep $((attempt * 3))
done
rm -rf "$OUT/dmg-root"
echo "Created $OUT/$DMG"
