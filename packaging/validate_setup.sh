#!/bin/zsh
# validate_setup.sh — Validate release setup and prerequisites
#
# This script checks that all required tools and configurations are in place
# for building, signing, and notarizing CodeScribe.

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "$0")/.." && pwd)"

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "CodeScribe Release Setup Validation"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

ERRORS=0
WARNINGS=0

# Check function
check_required() {
  local name="$1"
  local cmd="$2"
  if command -v "$cmd" >/dev/null 2>&1; then
    echo "✓ $name: $(command -v "$cmd")"
  else
    echo "✗ $name: NOT FOUND"
    ((ERRORS++))
  fi
}

check_optional() {
  local name="$1"
  local cmd="$2"
  if command -v "$cmd" >/dev/null 2>&1; then
    echo "✓ $name: $(command -v "$cmd")"
  else
    echo "⚠ $name: not found (optional)"
    ((WARNINGS++))
  fi
}

# Check required tools
echo "🔧 Required Tools"
echo "─────────────────"
check_required "cargo" "cargo"
check_required "codesign" "codesign"
check_required "hdiutil" "hdiutil"
check_required "xcrun" "xcrun"
check_required "security" "security"
check_required "rsync" "rsync"
echo ""

# Check optional tools
echo "🔧 Optional Tools"
echo "─────────────────"
check_optional "uv" "uv"
check_optional "git-lfs" "git-lfs"
echo ""

# Check Rust project
echo "🦀 Rust Project"
echo "───────────────"
if [[ -f "$ROOT_DIR/codescribe-rs/Cargo.toml" ]]; then
  VERSION=$(awk -F '"' '/^version[[:space:]]*=/{print $2; exit}' "$ROOT_DIR/codescribe-rs/Cargo.toml" 2>/dev/null || echo "unknown")
  echo "✓ Cargo.toml found"
  echo "  Version: $VERSION"
else
  echo "✗ Cargo.toml not found at codescribe-rs/Cargo.toml"
  ((ERRORS++))
fi
echo ""

# Check packaging files
echo "📦 Packaging Files"
echo "──────────────────"
FILES=(
  "packaging/release_master.sh"
  "packaging/dmg/build_dmg.sh"
  "packaging/appwrap/build_wrapper_app.sh"
  "packaging/scripts/sign_and_notarize.sh"
  "packaging/entitlements.plist"
)

for file in "${FILES[@]}"; do
  if [[ -f "$ROOT_DIR/$file" ]]; then
    if [[ -x "$ROOT_DIR/$file" || "$file" == *.plist ]]; then
      echo "✓ $file"
    else
      echo "⚠ $file (not executable)"
      ((WARNINGS++))
    fi
  else
    echo "✗ $file (missing)"
    ((ERRORS++))
  fi
done
echo ""

# Check signing certificate
echo "🔐 Code Signing"
echo "───────────────"
CERT_NAME="Developer ID Application: Maciej Gad (MW223P3NPX)"
if security find-identity -v -p codesigning | grep -q "$CERT_NAME"; then
  echo "✓ Certificate found: $CERT_NAME"
  CERT_INFO=$(security find-identity -v -p codesigning | grep "$CERT_NAME" | head -n1)
  echo "  $CERT_INFO"
else
  echo "⚠ Certificate not found: $CERT_NAME"
  echo "  Available certificates:"
  security find-identity -v -p codesigning | grep "Developer ID" || echo "  (none)"
  ((WARNINGS++))
fi
echo ""

# Check notarization profile
echo "📨 Notarization"
echo "───────────────"
PROFILE_NAME="Vista Notary"
if xcrun notarytool history --keychain-profile "$PROFILE_NAME" >/dev/null 2>&1; then
  echo "✓ Notarization profile found: $PROFILE_NAME"
else
  echo "⚠ Notarization profile not found: $PROFILE_NAME"
  echo "  Run this to create it:"
  echo "  xcrun notarytool store-credentials \"$PROFILE_NAME\" \\"
  echo "    --apple-id your@email.com \\"
  echo "    --team-id MW223P3NPX \\"
  echo "    --password app-specific-password"
  ((WARNINGS++))
fi
echo ""

# Check Whisper models
echo "🤖 Whisper Models"
echo "─────────────────"
MODEL_DIR="$ROOT_DIR/models"
if [[ -d "$MODEL_DIR" ]]; then
  MODELS=$(find "$MODEL_DIR" -maxdepth 1 -type d -name "whisper-*" -o -name "small" -o -name "medium" -o -name "large-*" 2>/dev/null | wc -l | tr -d ' ')
  if [[ "$MODELS" -gt 0 ]]; then
    echo "✓ Found $MODELS Whisper model(s) in $MODEL_DIR"
    find "$MODEL_DIR" -maxdepth 1 -type d \( -name "whisper-*" -o -name "small" -o -name "medium" -o -name "large-*" \) -exec basename {} \; 2>/dev/null | sed 's/^/  - /'
  else
    echo "⚠ No Whisper models found in $MODEL_DIR"
    echo "  The .app will ship without a bundled model"
    echo "  Users will need to download a model on first launch"
    ((WARNINGS++))
  fi
else
  echo "⚠ Models directory not found: $MODEL_DIR"
  ((WARNINGS++))
fi
echo ""

# Check output directory
echo "📁 Output Directory"
echo "───────────────────"
DIST_DIR="$ROOT_DIR/packaging/dist"
if [[ -d "$DIST_DIR" ]]; then
  echo "✓ Dist directory exists: $DIST_DIR"
  if [[ -d "$DIST_DIR/CodeScribe.app" ]]; then
    APP_SIZE=$(du -sh "$DIST_DIR/CodeScribe.app" | awk '{print $1}')
    echo "  Existing .app bundle found ($APP_SIZE)"
  fi
  if compgen -G "$DIST_DIR/CodeScribe-*.dmg" > /dev/null; then
    for dmg in "$DIST_DIR"/CodeScribe-*.dmg; do
      DMG_SIZE=$(du -sh "$dmg" | awk '{print $1}')
      echo "  Existing DMG found: $(basename "$dmg") ($DMG_SIZE)"
    done
  fi
else
  echo "⚠ Dist directory doesn't exist (will be created): $DIST_DIR"
fi
echo ""

# Summary
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [[ $ERRORS -eq 0 && $WARNINGS -eq 0 ]]; then
  echo "✅ All checks passed! Ready to build."
elif [[ $ERRORS -eq 0 ]]; then
  echo "⚠️  $WARNINGS warning(s) - Build possible but some features may be limited"
else
  echo "❌ $ERRORS error(s), $WARNINGS warning(s) - Fix errors before building"
fi
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

if [[ $ERRORS -gt 0 ]]; then
  exit 1
fi

exit 0
