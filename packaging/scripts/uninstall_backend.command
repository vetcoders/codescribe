#!/bin/zsh
# uninstall_backend.command
#
# Purpose: Stop and remove the VistaScribe backend LaunchAgent and logs.

set -euo pipefail

PLIST="$HOME/Library/LaunchAgents/com.VistaScribe.backend.plist"

if launchctl list | grep -q "com.VistaScribe.backend"; then
  echo "[i] Unloading LaunchAgent…"
  launchctl unload "$PLIST" || true
fi

if [[ -f "$PLIST" ]]; then
  echo "[i] Removing $PLIST"
  rm -f "$PLIST"
fi

echo "[i] Removing logs from /tmp"
rm -f /tmp/VistaScribe.backend.out.log || true
rm -f /tmp/VistaScribe.backend.err.log || true

echo "[✓] Backend uninstalled."
