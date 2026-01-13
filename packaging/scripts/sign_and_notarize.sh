#!/bin/zsh
# sign_and_notarize.sh — helper to codesign and notarize CodeScribe
# Usage:
#   packaging/scripts/sign_and_notarize.sh \
#     --app "packaging/dist/CodeScribe.app" \
#     --cert "Developer ID Application: Maciej Gad (MW223P3NPX)" \
#     --profile "Vista Notary" \
#     [--dmg packaging/dist/CodeScribe-0.5.0.dmg]

set -euo pipefail

APP=""
CERT="Developer ID Application: Maciej Gad (MW223P3NPX)"
PROFILE="Vista Notary"
DMG=""
ENT="$(cd -- "$(dirname "$0")/.." && pwd)/entitlements.plist"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) APP="$2"; shift 2;;
    --cert) CERT="$2"; shift 2;;
    --profile) PROFILE="$2"; shift 2;;
    --dmg) DMG="$2"; shift 2;;
    *) echo "Unknown arg: $1"; exit 1;;
  esac
done

if [[ -z "$APP" ]]; then
  echo "Usage: $0 --app <.app> [--cert <Developer ID>] [--profile <notarytool-profile>] [--dmg <.dmg>]" >&2
  exit 1
fi

if [[ ! -d "$APP" ]]; then
  echo "[!] App not found: $APP" >&2
  exit 1
fi

echo "[i] Codesigning app: $APP"
echo "    Certificate: $CERT"
echo "    Entitlements: $ENT"

# Sign inner binaries first if they exist
if [[ -f "$APP/Contents/MacOS/codescribe" ]]; then
  echo "[i] Signing launcher script"
  codesign --force --sign "$CERT" "$APP/Contents/MacOS/codescribe"
fi

# Sign the main bundle with hardened runtime and entitlements
codesign --deep --force --options runtime --entitlements "$ENT" --sign "$CERT" "$APP"

# Verify signing
echo "[i] Verifying codesign"
codesign --verify --deep --strict --verbose=2 "$APP"

if [[ -n "$DMG" && -f "$DMG" ]]; then
  echo "[i] Codesigning DMG: $DMG"
  codesign --force --sign "$CERT" "$DMG" || true
fi

if [[ -n "$PROFILE" ]]; then
  # Notarize the DMG if available, otherwise the app
  TARGET="${DMG:-}"

  # If no DMG specified, create a temporary ZIP for notarization
  if [[ -z "$TARGET" ]]; then
    ZIP_APP="${APP%/}.zip"
    echo "[i] Creating ZIP for notarization: $ZIP_APP"
    /usr/bin/ditto -c -k --keepParent "$APP" "$ZIP_APP"
    TARGET="$ZIP_APP"
  fi

  echo "[i] Submitting for notarization: $TARGET"
  echo "    Profile: $PROFILE"
  xcrun notarytool submit "$TARGET" --keychain-profile "$PROFILE" --wait

  echo "[i] Stapling ticket"
  if [[ -n "$DMG" && -f "$DMG" ]]; then
    xcrun stapler staple "$DMG"
    xcrun stapler validate "$DMG"
  fi

  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"

  # Cleanup temporary ZIP if created
  if [[ -n "${ZIP_APP:-}" && -f "$ZIP_APP" ]]; then
    rm -f "$ZIP_APP"
  fi

  echo "[i] Verifying Gatekeeper assessment"
  spctl --assess --type execute -vvvv "$APP" || true
else
  echo "[!] No notarization profile specified, skipping notarization"
fi

echo "[✓] Done."
