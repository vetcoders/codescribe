#!/bin/bash
# build_dmg.sh
#
# Purpose: Build a simple DMG for VistaScribe distribution.
# - Includes the packaged VistaScribe.app, an Applications alias, and a short README.
# - Creates VistaScribe-<version>-<timestamp>.dmg in packaging/dmg/
#
# Requirements: hdiutil (macOS), optional: create-dmg (not required)

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "$0")/../.." && pwd)"
OUT_DIR="$(cd -- "$(dirname "$0")" && pwd)"
STAGE_DIR="${OUT_DIR}/stage"

# Resolve version for DMG naming
VERSION="0.1.0"
if [[ -f "$ROOT_DIR/pyproject.toml" ]]; then
  VLINE=$(awk -F '"' '/^version[[:space:]]*=/{print $2; exit}' "$ROOT_DIR/pyproject.toml" 2>/dev/null || true)
  [[ -n "${VLINE:-}" ]] && VERSION="$VLINE"
fi
STAMP=$(date +%Y%m%d_%H%M%S)
DMG_NAME="VistaScribe-${VERSION}-${STAMP}.dmg"

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR"

# Copy app if built
APP_SRC="${ROOT_DIR}/packaging/dist/VistaScribe.app"
if [[ -d "$APP_SRC" ]]; then
  echo "[i] Adding app bundle: $APP_SRC"
  cp -R "$APP_SRC" "$STAGE_DIR/VistaScribe.app"
else
  echo "[!] App bundle not found at $APP_SRC — aborting."
  echo "    Build it first with: packaging/appwrap/build_wrapper_app.sh"
  exit 2
fi

# Applications symlink for drag-and-drop install UX
ln -sf /Applications "$STAGE_DIR/Applications"

# Minimal README inside DMG
cat >"$STAGE_DIR/README-INSTALL.txt" <<'TXT'
VistaScribe — Installation
==========================

1) Drag "VistaScribe.app" onto the "Applications" alias.
2) Launch VistaScribe from /Applications.
   • The tray will request Microphone, Accessibility, and Input Monitoring on first run.
   • A bundled Whisper model is preloaded; you can switch models later via Tray ▸ Models.
   • When you choose Quit… the app will ask whether to keep the background server running.
3) Check ~/Library/Logs/VistaScribe.app.log if you need to troubleshoot startup.

For background-server workflows and advanced settings, see README.md in the repo.
TXT

# Create DMG
DMG_PATH="${OUT_DIR}/${DMG_NAME}"
rm -f "$DMG_PATH"
hdiutil create -volname "VistaScribe" -srcfolder "$STAGE_DIR" -ov -format UDZO "$DMG_PATH"

# Optional: clear quarantine for local/internal testing (DMG_UNQUARANTINE=1)
if [[ "${DMG_UNQUARANTINE:-0}" == "1" ]]; then
  xattr -cr "$DMG_PATH" || true
fi

echo "[✓] Built DMG: $DMG_PATH"
