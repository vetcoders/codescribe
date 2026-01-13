#!/bin/bash
# CodeScribe Release Build Script
# Builds, signs, and packages CodeScribe.app
#
# Usage:
#   ./scripts/build-release.sh              # Ad-hoc signing (dev)
#   ./scripts/build-release.sh --sign       # Developer ID signing (prod)
#   ./scripts/build-release.sh --with-model # Include Whisper model (~900MB)
#
# Created by M&K (c)2026 VetCoders

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

# Configuration
APP_NAME="CodeScribe"
BUNDLE_ID="com.vetcoders.codescribe"
# Get version from [package] section (after line with "[package]")
VERSION=$(awk '/^\[package\]/{found=1} found && /^version/{print; exit}' Cargo.toml | sed 's/.*"\(.*\)"/\1/')
BUILD_DIR="target/release"
BUNDLE_DIR="bundle/${APP_NAME}.app"
ENTITLEMENTS="bundle/entitlements.plist"
MODEL_NAME="whisper-large-v3-turbo-mlx-q8"

# Parse arguments
SIGN_MODE="adhoc"
INCLUDE_MODEL=false
for arg in "$@"; do
    case $arg in
        --sign) SIGN_MODE="identity" ;;
        --with-model) INCLUDE_MODEL=true ;;
    esac
done

echo "═══════════════════════════════════════════════════════════"
echo "  CodeScribe Release Build v${VERSION}"
echo "═══════════════════════════════════════════════════════════"
echo "  Sign mode: ${SIGN_MODE}"
echo "  Include model: ${INCLUDE_MODEL}"
echo "───────────────────────────────────────────────────────────"

# Step 1: Build release binary
echo ""
echo "▶ Building release binary..."
cargo build --release
BINARY="${BUILD_DIR}/codescribe"

if [ ! -f "$BINARY" ]; then
    echo "✗ Build failed: binary not found"
    exit 1
fi

BINARY_SIZE=$(du -h "$BINARY" | cut -f1)
echo "  Binary: ${BINARY} (${BINARY_SIZE})"

# Step 2: Create app bundle structure
echo ""
echo "▶ Creating app bundle..."
rm -rf "${BUNDLE_DIR}"
mkdir -p "${BUNDLE_DIR}/Contents/MacOS"
mkdir -p "${BUNDLE_DIR}/Contents/Resources"

# Copy binary
cp "$BINARY" "${BUNDLE_DIR}/Contents/MacOS/${APP_NAME}"

# Copy icon
if [ -f "assets/AppIcon.icns" ]; then
    cp "assets/AppIcon.icns" "${BUNDLE_DIR}/Contents/Resources/"
fi

# Step 3: Include model if requested (heavy install)
if [ "$INCLUDE_MODEL" = true ]; then
    echo ""
    echo "▶ Including Whisper model (heavy install)..."
    MODEL_SRC="models/${MODEL_NAME}"
    MODEL_DST="${BUNDLE_DIR}/Contents/Resources/models/${MODEL_NAME}"

    if [ -d "$MODEL_SRC" ]; then
        mkdir -p "${BUNDLE_DIR}/Contents/Resources/models"
        cp -R "$MODEL_SRC" "$MODEL_DST"
        MODEL_SIZE=$(du -sh "$MODEL_DST" | cut -f1)
        echo "  Model bundled: ${MODEL_SIZE}"
    else
        echo "  ⚠ Model not found at ${MODEL_SRC}"
        echo "  Run: ./scripts/download-model.sh"
    fi
fi

# Step 4: Create Info.plist
echo ""
echo "▶ Creating Info.plist..."
cat > "${BUNDLE_DIR}/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>LSMinimumSystemVersion</key>
    <string>14.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>CodeScribe needs microphone access for speech-to-text transcription.</string>
    <key>NSAppleEventsUsageDescription</key>
    <string>CodeScribe uses accessibility to detect global hotkeys and paste transcriptions.</string>
</dict>
</plist>
EOF

# Step 5: Code signing
echo ""
echo "▶ Code signing..."

if [ "$SIGN_MODE" = "identity" ]; then
    # Find Developer ID signing identity
    IDENTITY=$(security find-identity -v -p codesigning | grep "Developer ID Application" | head -1 | sed 's/.*"\(.*\)".*/\1/' || true)

    if [ -z "$IDENTITY" ]; then
        # Fallback to Mac Developer
        IDENTITY=$(security find-identity -v -p codesigning | grep "Mac Developer" | head -1 | sed 's/.*"\(.*\)".*/\1/' || true)
    fi

    if [ -z "$IDENTITY" ]; then
        echo "  ⚠ No signing identity found, using ad-hoc"
        SIGN_MODE="adhoc"
    else
        echo "  Identity: ${IDENTITY}"
    fi
fi

if [ "$SIGN_MODE" = "adhoc" ]; then
    codesign --force --deep --options runtime \
        --entitlements "$ENTITLEMENTS" \
        --sign - \
        "${BUNDLE_DIR}"
    echo "  Signed: ad-hoc (development only)"
else
    codesign --force --deep --options runtime \
        --entitlements "$ENTITLEMENTS" \
        --timestamp \
        --sign "$IDENTITY" \
        "${BUNDLE_DIR}"
    echo "  Signed: ${IDENTITY}"
fi

# Step 6: Verify signature
echo ""
echo "▶ Verifying signature..."
codesign --verify --deep --strict --verbose=2 "${BUNDLE_DIR}" 2>&1 | head -5

# Step 7: Create DMG with Applications symlink
echo ""
echo "▶ Creating DMG..."
DMG_NAME="${APP_NAME}_${VERSION}_$(date +%Y%m%d).dmg"
DMG_TEMP="${APP_NAME}_temp.dmg"
rm -f "$DMG_NAME" "$DMG_TEMP"

# Calculate size (app + headroom for Applications alias)
if [ "$INCLUDE_MODEL" = true ]; then
    DMG_SIZE="1200m"
else
    DMG_SIZE="20m"
fi

# Create writable DMG
hdiutil create -size "$DMG_SIZE" -fs HFS+ -volname "${APP_NAME}" "$DMG_TEMP"

# Mount it
MOUNT_POINT=$(hdiutil attach "$DMG_TEMP" -nobrowse | tail -1 | awk '{print $3}')

# Copy app and create Applications symlink
cp -R "${BUNDLE_DIR}" "$MOUNT_POINT/"
ln -s /Applications "$MOUNT_POINT/Applications"

# Detach
hdiutil detach "$MOUNT_POINT" -quiet

# Convert to compressed read-only
hdiutil convert "$DMG_TEMP" -format UDZO -o "$DMG_NAME"
rm -f "$DMG_TEMP"

# Clean extended attributes
xattr -cr "$DMG_NAME" 2>/dev/null || true

DMG_SIZE=$(du -h "$DMG_NAME" | cut -f1)

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Build Complete!"
echo "═══════════════════════════════════════════════════════════"
echo "  App:     ${BUNDLE_DIR}"
echo "  DMG:     ${DMG_NAME} (${DMG_SIZE})"
echo "  Version: ${VERSION}"
echo ""
echo "  Next steps:"
if [ "$SIGN_MODE" = "identity" ]; then
    echo "    ./scripts/notarize.sh ${DMG_NAME}"
else
    echo "    For production: ./scripts/build-release.sh --sign"
fi
echo "───────────────────────────────────────────────────────────"
