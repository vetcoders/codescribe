#!/bin/zsh
set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "$0")/../.." && pwd)"
DIST_DIR="$ROOT_DIR/packaging/dist"
APP_DIR="$DIST_DIR/VistaScribe.app"

# Resolve version from pyproject.toml if present
VERSION="0.1.0"
if [[ -f "$ROOT_DIR/pyproject.toml" ]]; then
  VER_LINE=$(awk -F '"' '/^version[[:space:]]*=/{print $2; exit}' "$ROOT_DIR/pyproject.toml" 2>/dev/null || true)
  if [[ -n "${VER_LINE:-}" ]]; then VERSION="$VER_LINE"; fi
fi

echo "[i] Building wrapper app at: $APP_DIR"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources/Repo"

# Info.plist (minimal)
cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>VistaScribe</string>
  <key>CFBundleDisplayName</key><string>VistaScribe</string>
  <key>CFBundleIdentifier</key><string>com.vistascribe.app</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>vistascribe</string>
  <key>LSUIElement</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Needed to transcribe speech.</string>
  <key>NSAccessibilityUsageDescription</key><string>Needed to monitor hotkeys and paste results.</string>
</dict>
</plist>
PLIST

# Launcher executable
cat > "$APP_DIR/Contents/MacOS/vistascribe" <<'LAUNCH'
#!/bin/zsh
set -euo pipefail
APP_DIR="$(cd -- "$(dirname "$0")/.." && pwd)"
REPO_DIR="$APP_DIR/Contents/Resources/Repo"
LOG_DIR="$HOME/Library/Logs"
mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/VistaScribe.app.log"

export NOHUP_MODE=1
cd "$REPO_DIR"
# Start tray + backend in background, with logging; let quickstart manage PIDs
nohup ./scripts/quickstart_mac.sh --mode both --daemon --log "$LOG_FILE" >> "$LOG_FILE" 2>&1 &
exit 0
LAUNCH
chmod +x "$APP_DIR/Contents/MacOS/vistascribe"

# Copy repo (exclude heavy/irrelevant dirs)
echo "[i] Copying repo into app Resources (trimmed)"
rsync -a --delete \
  --exclude '.git/' --exclude '.venv/' --exclude 'models/' --exclude 'outputs/' \
  --exclude 'logs/' --exclude 'packaging/dist/' --exclude 'packaging/build/' \
  "$ROOT_DIR/" "$APP_DIR/Contents/Resources/Repo/"

echo "[✓] Wrapper app built: $APP_DIR"
