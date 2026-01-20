#!/usr/bin/env bash
# notarize.sh — Sign, notarize and staple CodeScribe for macOS distribution
# Usage: ./packaging/notarize.sh [--no-build]

set -euo pipefail
BLUE='\033[0;34m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
info(){ echo -e "${BLUE}ℹ️  $*${NC}"; }
ok(){ echo -e "${GREEN}✅ $*${NC}"; }
warn(){ echo -e "${YELLOW}⚠️  $*${NC}"; }
err(){ echo -e "${RED}❌ $*${NC}"; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

# Config
APP_NAME="CodeScribe"
NOTARY_PROFILE="Vista-Notary"
SIGN_ID="Developer ID Application: Maciej Gad (MW223P3NPX)"
ENTITLEMENTS="$SCRIPT_DIR/appwrap/entitlements.plist"
DIST_DIR="$SCRIPT_DIR/dist"

NO_BUILD=0
[[ "${1:-}" == "--no-build" ]] && NO_BUILD=1

# Build if needed
if [[ $NO_BUILD -eq 0 ]]; then
  info "Building CodeScribe.app..."
  "$SCRIPT_DIR/appwrap/build_wrapper_app.sh"
fi

APP_PATH="$DIST_DIR/$APP_NAME.app"
[[ ! -d "$APP_PATH" ]] && err "App not found: $APP_PATH"

# Get version for DMG naming
VERSION=$(grep '^version' "$ROOT_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)"/\1/')
DMG_PATH="$DIST_DIR/${APP_NAME}-${VERSION}.dmg"

# Sign nested binaries first
info "Codesigning nested binaries (hardened runtime)..."
while IFS= read -r f; do
  codesign --force --options runtime --timestamp \
    --entitlements "$ENTITLEMENTS" \
    --sign "$SIGN_ID" "$f" 2>/dev/null || true
done < <(find "$APP_PATH/Contents" \( -type f -perm -111 -o -name "*.dylib" -o -name "*.so" \) -print)

info "Codesigning app bundle..."
codesign --force --options runtime --timestamp \
  --entitlements "$ENTITLEMENTS" \
  --sign "$SIGN_ID" "$APP_PATH"

info "Verifying code signature..."
codesign --verify --deep --strict "$APP_PATH" || err "Signature verification failed"
ok "Code signature verified"

# Create signed DMG
info "Creating signed DMG..."
rm -f "$DMG_PATH" 2>/dev/null || true
TMP_DMG="$(mktemp -d)/dmg"
mkdir -p "$TMP_DMG"
cp -R "$APP_PATH" "$TMP_DMG/"
ln -s /Applications "$TMP_DMG/Applications"
hdiutil create -volname "$APP_NAME" -srcfolder "$TMP_DMG" -ov -format UDZO "$DMG_PATH"
codesign --force --sign "$SIGN_ID" "$DMG_PATH"

# Package for notarization
ZIP_PATH="$DIST_DIR/${APP_NAME}-${VERSION}.zip"
info "Packaging for notarization..."
rm -f "$ZIP_PATH" 2>/dev/null || true
ditto -c -k --keepParent "$APP_PATH" "$ZIP_PATH"

# Submit to Apple Notary
info "Submitting to Apple Notary Service..."
xcrun notarytool submit "$ZIP_PATH" \
  --keychain-profile "$NOTARY_PROFILE" \
  --wait --timeout 15m --progress

# Staple tickets
info "Stapling notarization ticket..."
xcrun stapler staple "$APP_PATH" || warn "Staple (app) issue"
xcrun stapler staple "$DMG_PATH" || warn "Staple (dmg) issue"

# Final verification
info "Gatekeeper assessment..."
spctl --assess --type execute --verbose=2 "$APP_PATH" && ok "Gatekeeper: APPROVED" || warn "Gatekeeper issues"

# Cleanup zip
rm -f "$ZIP_PATH"

ok "Notarization complete!"
echo ""
echo "📦 Ready for distribution:"
echo "   App: $APP_PATH"
echo "   DMG: $DMG_PATH ($(du -h "$DMG_PATH" | cut -f1))"
