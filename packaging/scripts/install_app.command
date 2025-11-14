#!/bin/zsh
set -euo pipefail

HERE="$(cd -- "$(dirname "$0")" && pwd)"
DMG_ROOT="$(cd -- "$HERE/.." && pwd)"
APP_SRC="$DMG_ROOT/../dist/VistaScribe.app"
if [[ ! -d "$APP_SRC" ]]; then
  # When executed from DMG: Helpers is sibling of the app bundle
  APP_SRC="$DMG_ROOT/../Vista Scribe.app"
fi

DEST="/Applications/VistaScribe.app"

echo "[i] Installing to $DEST"
if cp -R "$APP_SRC" "$DEST" 2>/dev/null; then
  :
else
  echo "[i] Using administrator privileges to copy to /Applications"
  /usr/bin/osascript -e "do shell script \"cp -R \""$APP_SRC"\" \""$DEST"\"\" with administrator privileges"
fi

# Remove quarantine attribute to avoid warning on first launch
if command -v xattr >/dev/null 2>&1; then
  xattr -dr com.apple.quarantine "$DEST" || true
fi

open "$DEST"
echo "[✓] Installed and launched."

