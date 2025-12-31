#!/bin/bash
# Create CodeScribe.dmg for distribution
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_DIR="$SCRIPT_DIR/dist"
APP_NAME="CodeScribe"
DMG_NAME="CodeScribe-$(grep '^version' "$ROOT_DIR/codescribe-rs/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/').dmg"

# Build the app first
echo "[i] Building CodeScribe.app..."
"$SCRIPT_DIR/appwrap/build_wrapper_app.sh"

# Create DMG
echo "[i] Creating DMG: $DMG_NAME"
DMG_PATH="$DIST_DIR/$DMG_NAME"
rm -f "$DMG_PATH"

# Create temporary DMG folder
TMP_DMG="$(mktemp -d)/dmg"
mkdir -p "$TMP_DMG"
cp -R "$DIST_DIR/$APP_NAME.app" "$TMP_DMG/"
ln -s /Applications "$TMP_DMG/Applications"

# Create DMG
hdiutil create -volname "$APP_NAME" -srcfolder "$TMP_DMG" -ov -format UDZO "$DMG_PATH"

echo "[✓] DMG created: $DMG_PATH"
echo "    Size: $(du -h "$DMG_PATH" | cut -f1)"
