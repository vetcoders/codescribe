#!/bin/zsh
# notary_quick.sh — one‑shot helper: import cert (optional), codesign, notarize, staple
#
# Prompts only for passwords not found on disk/keychain. Designed to be called
# from Automator/osascript so you can just hit Enter → type hasło → Enter.
#
# Usage examples:
#   packaging/scripts/notary_quick.sh \
#     --app packaging/dist/CodeScribe.app \
#     --dmg packaging/dmg/CodeScribe.dmg \
#     --apple-id you@example.com --team-id ABCDE12345
#
#   # If you already stored a notarytool profile (VSNotary):
#   packaging/scripts/notary_quick.sh --app packaging/dist/CodeScribe.app --profile VSNotary

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
SKIP_NOTARY=0

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
    --no-notary) SKIP_NOTARY=1; shift 1;;
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
if [[ -f "$APP/Contents/MacOS/codescribe" && -n "$IDENTITY" ]]; then
  # Sign the launcher script first (no entitlements; scripts aren't Mach-O)
  codesign --force --sign "$IDENTITY" "$APP/Contents/MacOS/codescribe"
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

if [[ "$SKIP_NOTARY" -eq 0 ]]; then
  # 3) Notarization
  # If we have a DMG, submit the DMG; additionally, submit the .app as a ZIP so it can be stapled too.
  ZIP_APP=""
  if [[ -d "$APP" ]]; then
    ZIP_APP="${APP%/}.zip"
    echo "[i] Creating ZIP for app notarization: $ZIP_APP"
    /usr/bin/ditto -c -k --keepParent "$APP" "$ZIP_APP"
  fi

submit_with_profile() {
  local file="$1"
  echo "📨 Submitting with notarytool profile: ${PROFILE:-$PROFILE_DEF} → $(basename "$file")"
  if [[ -n "$PROFILE" ]]; then
    xcrun notarytool submit "$file" --keychain-profile "$PROFILE" --wait
  else
    xcrun notarytool submit "$file" --keychain-profile "$PROFILE_DEF" --wait
  fi
}

submit_with_creds() {
  local file="$1"
  echo "📨 Submitting with Apple ID credentials → $(basename "$file")"
  xcrun notarytool submit "$file" --apple-id "$APPLE_ID" --team-id "$TEAM_ID" --password "$APP_PW" --wait
}

  if [[ -n "$PROFILE" ]]; then
  # Use the provided keychain profile
  [[ -n "$DMG" && -f "$DMG" ]] && submit_with_profile "$DMG"
  [[ -n "$ZIP_APP" && -f "$ZIP_APP" ]] && submit_with_profile "$ZIP_APP"
  else
  # Interactive credentials path
  if [[ -z "$APPLE_ID" ]]; then vared -p "Apple ID (e.g., you@example.com): " -c APPLE_ID; fi
  if [[ -z "$TEAM_ID" ]]; then vared -p "Team ID (10 chars): " -c TEAM_ID; fi
  read -s "APP_PW?App-specific password: "; echo
  [[ -n "$DMG" && -f "$DMG" ]] && submit_with_creds "$DMG"
  [[ -n "$ZIP_APP" && -f "$ZIP_APP" ]] && submit_with_creds "$ZIP_APP"
  fi

  echo "📎 Stapling ticket(s)"
  [[ -n "$DMG" && -f "$DMG" ]] && xcrun stapler staple "$DMG" || true
  [[ -d "$APP" ]] && xcrun stapler staple "$APP" || true
  [[ -n "$DMG" && -f "$DMG" ]] && xcrun stapler validate "$DMG" || true

  # Cleanup temporary ZIP if we created one
  [[ -n "$ZIP_APP" && -f "$ZIP_APP" ]] && rm -f "$ZIP_APP"

  echo "🔎 Verifying Gatekeeper assessment"
  spctl --assess --type execute -vvvv "$APP" || true

  echo "✅ Done. You can now distribute: ${DMG:-$APP}"
else
  echo "⏭️  Notarization skipped (--no-notary). Signed artifacts ready."
  codesign --display --verbose=1 "$APP" | sed -n '1,4p' || true
  [[ -n "$DMG" && -f "$DMG" ]] && codesign --display --verbose=1 "$DMG" | sed -n '1,4p' || true
  echo "➡️  APP: $APP"
  [[ -n "$DMG" && -f "$DMG" ]] && echo "➡️  DMG: $DMG"
fi
