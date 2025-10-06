#!/bin/zsh
# notary_quick.sh — one‑shot helper: import cert (optional), codesign, notarize, staple
#
# Prompts only for passwords not found on disk/keychain. Designed to be called
# from Automator/osascript so you can just hit Enter → type hasło → Enter.
#
# Usage examples:
#   packaging/scripts/notary_quick.sh \
#     --app packaging/dist/VistaScribe.app \
#     --dmg packaging/dmg/VistaScribe.dmg \
#     --apple-id you@example.com --team-id ABCDE12345
#
#   # If you already stored a notarytool profile (VSNotary):
#   packaging/scripts/notary_quick.sh --app packaging/dist/VistaScribe.app --profile VSNotary

set -euo pipefail

APP=""
DMG=""
IDENTITY=""
ENT="$(cd -- "$(dirname "$0")/.." && pwd)/entitlements.plist"
PROFILE=""
APPLE_ID=""
TEAM_ID=""
P12="${HOME}/.keys/Certificates.p12"
P12_PASS_FILE="${HOME}/.keys/cert_password.txt"
KEYCHAIN="vistabuild.keychain-db"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) APP="$2"; shift 2;;
    --dmg) DMG="$2"; shift 2;;
    --identity|--cert) IDENTITY="$2"; shift 2;;
    --entitlements) ENT="$2"; shift 2;;
    --profile) PROFILE="$2"; shift 2;;
    --apple-id) APPLE_ID="$2"; shift 2;;
    --team-id) TEAM_ID="$2"; shift 2;;
    --p12) P12="$2"; shift 2;;
    --p12-pass-file) P12_PASS_FILE="$2"; shift 2;;
    --keychain) KEYCHAIN="$2"; shift 2;;
    *) echo "Unknown arg: $1" >&2; exit 1;;
  esac
done

if [[ -z "$APP" || ! -d "$APP" ]]; then
  echo "❌ --app must point to an existing .app bundle" >&2
  exit 1
fi

echo "🏁 notary_quick.sh"
echo "App: $APP"
[[ -n "$DMG" ]] && echo "DMG: $DMG"

# 1) Import Developer ID cert if identity not yet available
if [[ -z "$IDENTITY" ]]; then
  if security find-identity -v -p codesigning | grep -q "Developer ID Application"; then
    IDENTITY="$(security find-identity -v -p codesigning | awk -F'"' '/Developer ID Application/{print $2; exit}')"
  elif security find-identity -v -p codesigning | grep -q "Mac Developer"; then
    IDENTITY="Mac Developer"
  else
    IDENTITY=""
  fi
fi

if [[ -z "$IDENTITY" ]]; then
  if [[ -f "$P12" ]]; then
    echo "🔐 Importing P12 from: $P12"
    if [[ -f "$P12_PASS_FILE" ]]; then
      P12_PASS="$(cat "$P12_PASS_FILE")"
    else
      read -s "P12_PASS?Enter P12 password: "
      echo
    fi
    security create-keychain -p "" "$KEYCHAIN" >/dev/null 2>&1 || true
    security unlock-keychain -p "" "$KEYCHAIN"
    security set-keychain-settings -lut 3600 "$KEYCHAIN"
    security import "$P12" -P "$P12_PASS" -k "$KEYCHAIN" -T /usr/bin/codesign -T /usr/bin/security || true
    security list-keychains -d user -s "$KEYCHAIN" login.keychain-db
    if security find-identity -v -p codesigning | grep -q "Developer ID Application"; then
      IDENTITY="$(security find-identity -v -p codesigning | awk -F'"' '/Developer ID Application/{print $2; exit}')"
    fi
  fi
fi

if [[ -z "$IDENTITY" ]]; then
  echo "⚠️  No signing identity found; using ad-hoc for local testing."
fi
echo "✍️  Using identity: ${IDENTITY:-AD-HOC}"

# 2) Codesign (inner launcher first, then bundle)
if [[ -f "$APP/Contents/MacOS/vistascribe" && -n "$IDENTITY" ]]; then
  # Sign the launcher script first (no entitlements; scripts aren't Mach-O)
  codesign --force --sign "$IDENTITY" "$APP/Contents/MacOS/vistascribe"
fi

if [[ -n "$IDENTITY" ]]; then
  # Then sign the .app bundle with entitlements and hardened runtime
  codesign --deep --force --options runtime --entitlements "$ENT" --sign "$IDENTITY" "$APP"
else
  codesign --deep --force --options runtime --sign - "$APP"
fi

codesign --verify --deep --strict --verbose=2 "$APP"

if [[ -n "$DMG" && -f "$DMG" && -n "$IDENTITY" ]]; then
  echo "[i] Codesigning DMG"
  codesign --force --sign "$IDENTITY" "$DMG" || true
fi

# 3) Notarization
TARGET="${DMG:-$APP}"
if [[ -n "$PROFILE" ]]; then
  echo "📨 Submitting with notarytool profile: $PROFILE"
  xcrun notarytool submit "$TARGET" --keychain-profile "$PROFILE" --wait
else
  # Use stored profile if exists
  if xcrun notarytool list-profiles >/dev/null 2>&1; then
    PROFILE_DEF="$(xcrun notarytool list-profiles | awk 'NR==2{print $1}')"
  else
    PROFILE_DEF=""
  fi
  if [[ -n "$PROFILE_DEF" ]]; then
    echo "📨 Submitting with detected profile: $PROFILE_DEF"
    xcrun notarytool submit "$TARGET" --keychain-profile "$PROFILE_DEF" --wait
  else
    # Interactive credentials
    if [[ -z "$APPLE_ID" ]]; then
      vared -p "Apple ID (e.g., you@example.com): " -c APPLE_ID
    fi
    if [[ -z "$TEAM_ID" ]]; then
      vared -p "Team ID (10 chars): " -c TEAM_ID
    fi
    read -s "APP_PW?App-specific password: "
    echo
    echo "📨 Submitting with Apple ID credentials"
    xcrun notarytool submit "$TARGET" --apple-id "$APPLE_ID" --team-id "$TEAM_ID" --password "$APP_PW" --wait
  fi
fi

echo "📎 Stapling ticket"
xcrun stapler staple "$TARGET" || true

echo "🔎 Verifying Gatekeeper assessment"
spctl --assess --type execute -vvvv "$APP" || true
[[ -n "$DMG" && -f "$DMG" ]] && xcrun stapler validate "$DMG" || true

echo "✅ Done. You can now distribute: $TARGET"
