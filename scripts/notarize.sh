#!/bin/bash
# Codescribe Notarization Script
# Submits app to Apple for notarization and staples the ticket
#
# Prerequisites:
#   - Apple Developer account
#   - App-specific password (generate at appleid.apple.com)
#   - Store credentials: xcrun notarytool store-credentials "NOTARY_PROFILE" \
#       --apple-id "your@email.com" --team-id "TEAMID" --password "app-specific-pwd"
#
# Usage:
#   ./scripts/notarize.sh --app <signed-app-path>
#   ./scripts/notarize.sh Codescribe-<VERSION>.dmg [app-path]
#   NOTARY_PROFILE=MyProfile ./scripts/notarize.sh Codescribe.dmg
#
# The optional second argument points at the already-stapled .app to verify. It
# defaults to the Xcode-built product (macos/build/Build/Products/Release), the
# same bundle build-dmg.sh packages — the old bundle/ hand-built path is retired.
#
# Created by Vetcoders (c)2026

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

# Configuration
NOTARY_PROFILE="${NOTARY_PROFILE:-VSNotary}"
APP_NAME="${CODESCRIBE_APP_NAME:-Codescribe}"
DEFAULT_BUNDLE_DIR="$PROJECT_DIR/macos/build/Build/Products/Release/${APP_NAME}.app"

# Parse arguments
if [ $# -lt 1 ]; then
    echo "Usage: $0 <dmg_file>"
    echo ""
    echo "Environment variables:"
    echo "  NOTARY_PROFILE  - Keychain profile name (default: ${NOTARY_PROFILE})"
    echo ""
    echo "Setup credentials first:"
    echo "  xcrun notarytool store-credentials \"${NOTARY_PROFILE}\" \\"
    echo "      --apple-id \"your@email.com\" \\"
    echo "      --team-id \"TEAMID\" \\"
    echo "      --password \"app-specific-password\""
    exit 1
fi

APP_ONLY=0
if [[ "$1" == --app ]]; then
    [[ $# -eq 2 ]] || { echo "Usage: $0 --app <signed-app-path>" >&2; exit 2; }
    APP_ONLY=1
    BUNDLE_DIR="$2"
    DMG_FILE=""
else
    [[ $# -le 2 ]] || { echo "Usage: $0 <dmg_file> [stapled-app-path]" >&2; exit 2; }
    DMG_FILE="$1"
    BUNDLE_DIR="${2:-$DEFAULT_BUNDLE_DIR}"
fi

if [[ "$APP_ONLY" -eq 0 && ! -f "$DMG_FILE" ]]; then
    echo "✗ DMG not found: $DMG_FILE"
    exit 1
fi

if [ ! -d "$BUNDLE_DIR" ]; then
    echo "✗ App bundle not found: $BUNDLE_DIR"
    echo "  Build it first: make release-standard (build-dmg.sh --sign)"
    exit 1
fi

echo "═══════════════════════════════════════════════════════════"
echo "  Codescribe Notarization"
echo "═══════════════════════════════════════════════════════════"
echo "  App:     ${BUNDLE_DIR}"
if [[ "$APP_ONLY" -eq 0 ]]; then echo "  DMG:     ${DMG_FILE}"; fi
echo "  Profile: ${NOTARY_PROFILE}"
echo "───────────────────────────────────────────────────────────"

# Step 1: Verify app is properly signed
echo ""
echo "▶ Verifying code signature..."
if ! codesign --verify --deep --strict "${BUNDLE_DIR}" 2>/dev/null; then
    echo "✗ App not properly signed. Run 'make release-standard' (build-dmg.sh --sign) first"
    exit 1
fi
echo "  ✓ Signature valid"

NOTARY_ARCHIVE_DIR=""
trap 'if [[ -n "$NOTARY_ARCHIVE_DIR" ]]; then rm -rf "$NOTARY_ARCHIVE_DIR"; fi' EXIT
if [[ "$APP_ONLY" -eq 1 ]]; then
    NOTARY_ARCHIVE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/codescribe-notary.XXXXXXXX")"
    SUBMIT_FILE="$NOTARY_ARCHIVE_DIR/${APP_NAME}.zip"
    ditto -c -k --keepParent "$BUNDLE_DIR" "$SUBMIT_FILE"
else
    if ! xcrun stapler validate "$BUNDLE_DIR"; then
        echo "✗ App must be stapled before creating the DMG. Rebuild with make release-standard." >&2
        exit 1
    fi
    SUBMIT_FILE="$DMG_FILE"
fi

# Step 2: Submit for notarization
echo ""
echo "▶ Submitting to Apple Notary Service..."
echo "  This may take 5-15 minutes..."

set +e
SUBMIT_OUTPUT=$(xcrun notarytool submit "$SUBMIT_FILE" \
    --keychain-profile "$NOTARY_PROFILE" \
    --wait \
    --timeout 30m 2>&1)
SUBMIT_EXIT=$?
set -e

echo "$SUBMIT_OUTPUT"

if [ $SUBMIT_EXIT -ne 0 ]; then
    echo ""
    echo "✗ Notary submission command failed (exit: $SUBMIT_EXIT)"
    if echo "$SUBMIT_OUTPUT" | grep -qi "Invalid credentials"; then
        echo "  Hint: refresh profile '${NOTARY_PROFILE}' via:"
        echo "    xcrun notarytool store-credentials \"${NOTARY_PROFILE}\" --apple-id ... --team-id ... --password ..."
    fi
    exit $SUBMIT_EXIT
fi

# Check if notarization succeeded
if echo "$SUBMIT_OUTPUT" | grep -Eiq "status[^A-Za-z]*Accepted"; then
    echo ""
    echo "  ✓ Notarization accepted!"
else
    echo ""
    echo "✗ Notarization failed"

    # Get submission ID for log retrieval
    SUBMISSION_ID=$(echo "$SUBMIT_OUTPUT" | grep "id:" | head -1 | awk '{print $2}')
    if [ -n "$SUBMISSION_ID" ]; then
        echo ""
        echo "▶ Fetching notarization log..."
        xcrun notarytool log "$SUBMISSION_ID" --keychain-profile "$NOTARY_PROFILE"
    fi
    exit 1
fi

# Step 3: Staple the notarization ticket
echo ""
echo "▶ Stapling notarization ticket..."

if [[ "$APP_ONLY" -eq 1 ]]; then
    xcrun stapler staple "$BUNDLE_DIR"
    xcrun stapler validate "$BUNDLE_DIR"
    echo "  ✓ App stapled before packaging: $BUNDLE_DIR"
else
    xcrun stapler staple "$DMG_FILE"
    xcrun stapler validate "$DMG_FILE"
    echo "  ✓ Stapled to ${DMG_FILE}"
fi

# Stapling rewrites the DMG bytes, so the .sha256 build-dmg.sh wrote at create
# time no longer matches — refresh it or the published checksum is a lie.
if [[ "$APP_ONLY" -eq 0 && -f "${DMG_FILE}.sha256" ]]; then
  (
    cd "$(dirname "$DMG_FILE")"
    shasum -a 256 "$(basename "$DMG_FILE")" > "$(basename "$DMG_FILE").sha256"
  )
  echo "  ✓ Refreshed ${DMG_FILE}.sha256 (post-staple bytes)"
fi

# Step 4: Verify Gatekeeper acceptance
echo ""
echo "▶ Verifying Gatekeeper acceptance..."
spctl --assess --type execute --verbose=4 "${BUNDLE_DIR}" 2>&1 | head -3

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Notarization Complete!"
echo "═══════════════════════════════════════════════════════════"
if [[ "$APP_ONLY" -eq 1 ]]; then
    echo "  App ready to package: ${BUNDLE_DIR}"
else
    echo "  DMG ready for distribution: ${DMG_FILE}"
fi
echo ""
echo "  Users can now install without Gatekeeper warnings."
echo "───────────────────────────────────────────────────────────"
