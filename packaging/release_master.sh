#!/bin/zsh
# release_master.sh — Master release script for CodeScribe
#
# This script orchestrates the complete release process:
# 1. Build Rust binary (release mode)
# 2. Create .app bundle
# 3. Sign and notarize .app
# 4. Create DMG
# 5. Sign and notarize DMG
#
# Usage:
#   packaging/release_master.sh                    # Build only (no signing)
#   packaging/release_master.sh --sign             # Build and sign (with notarization)
#   packaging/release_master.sh --sign --no-notary # Build and sign (skip notarization)
#
# Environment variables:
#   CERT           - Override default certificate (default: "Developer ID Application: Maciej Gad (MW223P3NPX)")
#   PROFILE        - Override default notarization profile (default: "Vista Notary")
#   SKIP_BUILD     - Set to 1 to skip Rust build step (useful for re-packaging)
#   SKIP_APP       - Set to 1 to skip app bundle creation
#   SKIP_DMG       - Set to 1 to skip DMG creation
#
# Examples:
#   SKIP_BUILD=1 packaging/release_master.sh --sign          # Re-sign existing build
#   CERT="Mac Developer" packaging/release_master.sh --sign  # Use different cert

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "$0")/.." && pwd)"
DIST_DIR="$ROOT_DIR/packaging/dist"

# Default settings
CERT="${CERT:-Developer ID Application: Maciej Gad (MW223P3NPX)}"
PROFILE="${PROFILE:-Vista Notary}"
DO_SIGN=0
NO_NOTARY=0
SKIP_BUILD="${SKIP_BUILD:-0}"
SKIP_APP="${SKIP_APP:-0}"
SKIP_DMG="${SKIP_DMG:-0}"

# Parse arguments
while [[ $# -gt 0 ]]; do
  case "$1" in
    --sign) DO_SIGN=1; shift;;
    --no-notary) NO_NOTARY=1; shift;;
    --skip-build) SKIP_BUILD=1; shift;;
    --skip-app) SKIP_APP=1; shift;;
    --skip-dmg) SKIP_DMG=1; shift;;
    *) echo "Unknown arg: $1"; exit 1;;
  esac
done

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "CodeScribe Release Master Script"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Step 1: Build Rust binary
if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "🦀 [1/5] Building Rust binary (release mode)..."
  cd "$ROOT_DIR/codescribe-rs"
  cargo build --release
  echo "✓ Rust binary built: target/release/codescribe"
  echo ""
else
  echo "⏭  [1/5] Skipping Rust build (SKIP_BUILD=1)"
  echo ""
fi

# Step 2: Create .app bundle
if [[ "$SKIP_APP" -eq 0 ]]; then
  echo "📦 [2/5] Creating .app bundle..."
  "$ROOT_DIR/packaging/appwrap/build_wrapper_app.sh"
  echo ""
else
  echo "⏭  [2/5] Skipping .app bundle creation (SKIP_APP=1)"
  echo ""
fi

APP_PATH="$DIST_DIR/CodeScribe.app"

# Step 3: Sign .app bundle
if [[ "$DO_SIGN" -eq 1 ]]; then
  echo "✍️  [3/5] Signing .app bundle..."
  SIGN_ARGS=(--app "$APP_PATH" --cert "$CERT")

  # Only add profile if notarization is requested
  if [[ "$NO_NOTARY" -eq 0 ]]; then
    SIGN_ARGS+=(--profile "$PROFILE")
  fi

  "$ROOT_DIR/packaging/scripts/sign_and_notarize.sh" "${SIGN_ARGS[@]}"
  echo ""
else
  echo "⏭  [3/5] Skipping signing (use --sign to enable)"
  echo ""
fi

# Step 4: Create DMG
if [[ "$SKIP_DMG" -eq 0 ]]; then
  echo "💿 [4/5] Creating DMG..."
  "$ROOT_DIR/packaging/dmg/build_dmg.sh"
  echo ""
else
  echo "⏭  [4/5] Skipping DMG creation (SKIP_DMG=1)"
  echo ""
fi

# Find the created DMG
DMG_PATH=$(ls -1t "$DIST_DIR"/CodeScribe-*.dmg 2>/dev/null | head -n1 || echo "")

# Step 5: Sign and notarize DMG
if [[ "$DO_SIGN" -eq 1 && -n "$DMG_PATH" && -f "$DMG_PATH" ]]; then
  echo "✍️  [5/5] Signing DMG..."
  SIGN_ARGS=(--app "$APP_PATH" --dmg "$DMG_PATH" --cert "$CERT")

  if [[ "$NO_NOTARY" -eq 0 ]]; then
    SIGN_ARGS+=(--profile "$PROFILE")
  fi

  "$ROOT_DIR/packaging/scripts/sign_and_notarize.sh" "${SIGN_ARGS[@]}"
  echo ""
else
  echo "⏭  [5/5] Skipping DMG signing"
  echo ""
fi

# Summary
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ Release Complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Output files in: $DIST_DIR"
echo ""

if [[ -d "$APP_PATH" ]]; then
  APP_SIZE=$(du -sh "$APP_PATH" | awk '{print $1}')
  echo "  .app bundle: $APP_PATH ($APP_SIZE)"
fi

if [[ -n "$DMG_PATH" && -f "$DMG_PATH" ]]; then
  DMG_SIZE=$(du -sh "$DMG_PATH" | awk '{print $1}')
  echo "  DMG:         $DMG_PATH ($DMG_SIZE)"
fi

echo ""

if [[ "$DO_SIGN" -eq 1 ]]; then
  echo "Signed with:     $CERT"
  if [[ "$NO_NOTARY" -eq 0 ]]; then
    echo "Notarized with:  $PROFILE"
  else
    echo "Notarization:    Skipped (--no-notary)"
  fi
else
  echo "⚠️  Not signed. Use --sign to enable signing and notarization."
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
