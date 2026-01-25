#!/bin/zsh
# install_backend.command
#
# Purpose: Install and start the CodeScribe backend (FastAPI) as a LaunchAgent.
# NOTE: The Rust app uses local Whisper by default. This backend is legacy.
# - Downloads/updates Whisper models into ~/.CodeScribe/models
# - Seeds the shared settings store so the backend follows the same AI provider/toggle as the tray
# - Writes ~/Library/LaunchAgents/com.CodeScribe.backend.plist and loads it via launchctl
#
# Usage: double-click in Finder (Terminal will open) or run from shell.
# Optional flags / env vars:
#   --variant <medium|large-v3-turbo|...>   (WHISPER_VARIANT)
#   --host <ip> --port <port>               (HOST/PORT for the backend service)

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
APP_SUPPORT="$HOME/.CodeScribe"
MODELS_DIR="$APP_SUPPORT/models"

WHISPER_VARIANT="${WHISPER_VARIANT:-medium}"
HOST="${HOST:-127.0.0.1}"
PORT="${PORT:-8237}"
LOG_LEVEL="${LOG_LEVEL:-INFO}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --variant) WHISPER_VARIANT="$2"; shift 2;;
    --host) HOST="$2"; shift 2;;
    --port) PORT="$2"; shift 2;;
    --log-level) LOG_LEVEL="$2"; shift 2;;
    -h|--help)
      echo "Usage: install_backend.command [--variant medium] [--host 127.0.0.1 --port 8237]"
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

lower_users() {
  local p="$1"
  if [[ "$p" == /Users/* ]]; then
    local fixed="/users/${p#*/Users/}"
    if [[ -e "$fixed" ]]; then
      echo "$fixed"
      return
    fi
  fi
  echo "$p"
}

if ! command -v uv >/dev/null 2>&1; then
  echo "[!] 'uv' not found. Install it first: curl -LsSf https://astral.sh/uv/install.sh | sh"
  exit 1
fi

mkdir -p "$MODELS_DIR"
mkdir -p "$APP_SUPPORT"

echo "[i] Ensuring Whisper model (${WHISPER_VARIANT}) is available under $MODELS_DIR"
(cd "$REPO_DIR" && uv run python scripts/get_models.py --whisper "$WHISPER_VARIANT" --models-dir "$MODELS_DIR")

WHISPER_DIR="$MODELS_DIR/whisper-${WHISPER_VARIANT}"
if [[ ! -d "$WHISPER_DIR" ]]; then
  echo "[!] Expected model not found at $WHISPER_DIR. Listing available models:"
  ls "$MODELS_DIR" || true
  echo "    Set WHISPER_VARIANT to one of the downloaded folders."
  exit 3
fi
WHISPER_DIR="$(lower_users "$WHISPER_DIR")"

PLIST="$HOME/Library/LaunchAgents/com.CodeScribe.backend.plist"
mkdir -p "$(dirname "$PLIST")"

cat >"$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>com.CodeScribe.backend</string>
    <key>ProgramArguments</key>
    <array>
      <string>/usr/bin/env</string>
      <string>bash</string>
      <string>-lc</string>
      <string>cd "$(lower_users "$REPO_DIR")" && uv run python -m codescribe.codescribe_server start --bind ${HOST} --port ${PORT} --log-level ${LOG_LEVEL}</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>EnvironmentVariables</key>
    <dict>
      <key>WHISPER_DIR</key><string>${WHISPER_DIR}</string>
      <key>HOST</key><string>${HOST}</string>
      <key>PORT</key><string>${PORT}</string>
      <key>LOG_LEVEL</key><string>${LOG_LEVEL}</string>
    </dict>
    <key>StandardOutPath</key><string>/tmp/CodeScribe.backend.out.log</string>
    <key>StandardErrorPath</key><string>/tmp/CodeScribe.backend.err.log</string>
  </dict>
</plist>
PLIST

echo "[i] LaunchAgent written to: $PLIST"

if launchctl list | grep -q "com.CodeScribe.backend"; then
  launchctl unload "$PLIST" || true
fi
launchctl load "$PLIST"
launchctl start com.CodeScribe.backend || true

echo "[✓] Backend running on ${HOST}:${PORT} (LaunchAgent: com.CodeScribe.backend)"
echo "    Logs: /tmp/CodeScribe.backend.{out,err}.log"
echo "    Settings: $SETTINGS_PATH"
echo "    Health check: curl -s http://${HOST}:${PORT}/healthz | jq"
