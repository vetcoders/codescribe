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
#   ./scripts/notarize.sh Codescribe-<VERSION>.dmg
#   NOTARY_PROFILE=MyProfile ./scripts/notarize.sh Codescribe.dmg
#
# Created by Vetcoders (c)2026

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

# Configuration
NOTARY_PROFILE="${NOTARY_PROFILE:-VSNotary}"
APP_NAME="${CODESCRIBE_APP_NAME:-Codescribe}"
BUNDLE_DIR="bundle/${APP_NAME}.app"

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

DMG_FILE="$1"

if [ ! -f "$DMG_FILE" ]; then
    echo "✗ DMG not found: $DMG_FILE"
    exit 1
fi

echo "═══════════════════════════════════════════════════════════"
echo "  Codescribe Notarization"
echo "═══════════════════════════════════════════════════════════"
echo "  DMG:     ${DMG_FILE}"
echo "  Profile: ${NOTARY_PROFILE}"
echo "───────────────────────────────────────────────────────────"

# Step 1: Verify app is properly signed
echo ""
echo "▶ Verifying code signature..."
if ! codesign --verify --deep --strict "${BUNDLE_DIR}" 2>/dev/null; then
    echo "✗ App not properly signed. Run build-release.sh --sign first"
    exit 1
fi
echo "  ✓ Signature valid"

# Step 2: Submit for notarization
echo ""
echo "▶ Submitting to Apple Notary Service..."
echo "  This may take 5-15 minutes..."

set +e
SUBMIT_OUTPUT=$(xcrun notarytool submit "$DMG_FILE" \
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

# Staple to app
xcrun stapler staple "${BUNDLE_DIR}"
echo "  ✓ Stapled to ${APP_NAME}.app"

# Staple to DMG
xcrun stapler staple "$DMG_FILE"
echo "  ✓ Stapled to ${DMG_FILE}"

# Step 4: Verify Gatekeeper acceptance
echo ""
echo "▶ Verifying Gatekeeper acceptance..."
spctl --assess --type execute --verbose=4 "${BUNDLE_DIR}" 2>&1 | head -3

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Notarization Complete!"
echo "═══════════════════════════════════════════════════════════"
echo "  DMG ready for distribution: ${DMG_FILE}"
echo ""
echo "  Users can now install without Gatekeeper warnings."
echo "───────────────────────────────────────────────────────────"
