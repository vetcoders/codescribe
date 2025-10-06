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
  <key>CFBundleIconFile</key><string>AppIcon</string>
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

# Ensure uv and brew binaries are on PATH when launched from Finder
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

export NOHUP_MODE=1
cd "$REPO_DIR"
# Run tray+backend in foreground so the app process stays alive
# All output goes to the app log for debugging if needed
exec ./scripts/quickstart_mac.sh --mode both --no-models --fg >> "$LOG_FILE" 2>&1
LAUNCH
chmod +x "$APP_DIR/Contents/MacOS/vistascribe"

# Build a basic .icns from assets/icon.png (best-effort)
ICON_SRC="$ROOT_DIR/assets/icon.png"
if [[ -f "$ICON_SRC" ]]; then
  echo "[i] Generating AppIcon.icns from assets/icon.png"
  ICONSET_DIR="$(mktemp -d)/AppIcon.iconset"
  mkdir -p "$ICONSET_DIR"
  for sz in 16 32 64 128 256 512; do
    /usr/bin/sips -z $sz $sz "$ICON_SRC" --out "$ICONSET_DIR/icon_${sz}x${sz}.png" >/dev/null 2>&1 || true
    /usr/bin/sips -z $((sz*2)) $((sz*2)) "$ICON_SRC" --out "$ICONSET_DIR/icon_${sz}x${sz}@2x.png" >/dev/null 2>&1 || true
  done
  /usr/bin/iconutil -c icns "$ICONSET_DIR" -o "$APP_DIR/Contents/Resources/AppIcon.icns" || true
fi

# Copy repo (exclude heavy/irrelevant dirs)
echo "[i] Copying repo into app Resources (trimmed)"
rsync -a --delete \
  --exclude '.git/' --exclude '.venv/' --exclude 'models/' --exclude 'outputs/' \
  --exclude 'logs/' --exclude 'packaging/dist/' --exclude 'packaging/build/' \
  --exclude 'packaging/dmg/' \
  "$ROOT_DIR/" "$APP_DIR/Contents/Resources/Repo/"

echo "[✓] Wrapper app built: $APP_DIR"
