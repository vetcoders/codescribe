#!/bin/zsh
# uninstall_backend.command
#
# Purpose: Stop and remove the CodeScribe backend LaunchAgent and logs.

set -euo pipefail

PLIST="$HOME/Library/LaunchAgents/com.CodeScribe.backend.plist"

if launchctl list | grep -q "com.CodeScribe.backend"; then
  echo "[i] Unloading LaunchAgent…"
  launchctl unload "$PLIST" || true
fi

if [[ -f "$PLIST" ]]; then
  echo "[i] Removing $PLIST"
  rm -f "$PLIST"
fi

echo "[i] Removing logs from /tmp"
rm -f /tmp/CodeScribe.backend.out.log || true
rm -f /tmp/CodeScribe.backend.err.log || true

echo "[✓] Backend uninstalled."
