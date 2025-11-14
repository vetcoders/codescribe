#!/bin/zsh
# sign_and_notarize.sh — helper to codesign and notarize VistaScribe
# Usage:
#   packaging/scripts/sign_and_notarize.sh \
#     --app "packaging/dist/VistaScribe.app" \
#     --cert "Developer ID Application: Your Name (TEAMID)" \
#     --profile "AC_PROFILE_NAME" \
#     [--dmg packaging/dmg/VistaScribe.dmg]

set -euo pipefail

APP=""
CERT=""
PROFILE=""
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

if [[ -z "$APP" || -z "$CERT" ]]; then
  echo "Usage: $0 --app <.app> --cert <Developer ID> [--profile <notarytool-profile>] [--dmg <.dmg>]" >&2
  exit 1
fi

echo "[i] Codesigning app: $APP"
codesign --deep --force --options runtime --entitlements "$ENT" --sign "$CERT" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

if [[ -n "$DMG" ]]; then
  echo "[i] Codesigning DMG: $DMG"
  codesign --force --sign "$CERT" "$DMG" || true
fi

if [[ -n "$PROFILE" ]]; then
  TARGET="${DMG:-$APP}"
  echo "[i] Submitting for notarization: $TARGET"
  xcrun notarytool submit "$TARGET" --keychain-profile "$PROFILE" --wait
  echo "[i] Stapling: $TARGET"
  xcrun stapler staple "$TARGET"
fi

echo "[✓] Done."

