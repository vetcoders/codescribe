#!/bin/zsh
# Double-click / `open -a Terminal` wrapper: runs the laptop sender under a GUI
# process so the microphone TCC grant applies (ssh-launched capture is silent).
cd "$(dirname "$0")/.." || exit 1
exec scripts/mic-bridge.sh send "${MIC_BRIDGE_DEST:-100.82.232.70}"
