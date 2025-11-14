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
MODELS_DST="$APP_DIR/Contents/Resources/Models"
mkdir -p "$MODELS_DST"

bundle_whisper_model() {
  local variant="$1"
  local src=""
  for candidate in \
    "$ROOT_DIR/models/whisper-${variant}" \
    "$ROOT_DIR/models/${variant}"; do
    if [[ -d "$candidate" ]]; then
      src="$candidate"
      break
    fi
  done
  [[ -z "$src" ]] && return 1
  local dst="$MODELS_DST/whisper-${variant}"
  echo "[i] Embedding model: $src → $dst"
  rm -rf "$dst"
  rsync -a "$src/" "$dst/"
  echo "$dst" >"$MODELS_DST/.last_whisper_variant"
  return 0
}

BUNDLE_VARIANTS=(${BUNDLE_VARIANTS:-medium large-v3-turbo small-mlx small})

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
  <key>NSInputMonitoringUsageDescription</key><string>Needed to detect keyboard shortcuts for recording.</string>
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
MODELS_DIR="$APP_DIR/Contents/Resources/Models"

# Ensure the shared settings live outside the app bundle
export VISTASCRIBE_SETTINGS_PATH="$HOME/.VistaScribe/settings.json"

# Ensure uv and brew binaries are on PATH when launched from Finder
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

# Prefer bundled Whisper models when present
if [[ -z "${WHISPER_DIR:-}" ]]; then
  if [[ -f "$MODELS_DIR/.last_whisper_variant" ]]; then
    candidate="$(cat "$MODELS_DIR/.last_whisper_variant" 2>/dev/null || true)"
    if [[ -n "$candidate" && -d "$candidate" ]]; then
      export WHISPER_DIR="$candidate"
    fi
  fi
  if [[ -z "${WHISPER_DIR:-}" ]]; then
    for d in "$MODELS_DIR"/whisper-*; do
      [[ -d "$d" ]] || continue
      export WHISPER_DIR="$d"
      break
    done
  fi
  if [[ -n "${WHISPER_DIR:-}" ]]; then
    export WHISPER_VARIANT="$(basename "$WHISPER_DIR" | sed 's/^whisper-//')"
  fi
fi

export NOHUP_MODE=1
cd "$REPO_DIR"
# Run tray+backend in foreground so the app process stays alive
# All output goes to the app log for debugging if needed
exec ./scripts/quickstart_mac.sh --mode both --fg >> "$LOG_FILE" 2>&1
LAUNCH
chmod +x "$APP_DIR/Contents/MacOS/vistascribe"

# Build a basic .icns from assets/icon.png (best-effort)
ICON_SRC="$ROOT_DIR/assets/icon.png"
if [[ ! -f "$ICON_SRC" ]]; then
  ICON_SRC="$ROOT_DIR/src/vistascribe/assets/icon.png"
fi
if [[ ! -f "$ICON_SRC" ]]; then
  ICON_SRC="$ROOT_DIR/Resources/assets/icon.png"
fi
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

echo "[i] Bundling Whisper model(s) for offline start"
BUNDLED_MODEL_OK=0
for variant in "${BUNDLE_VARIANTS[@]}"; do
  if bundle_whisper_model "$variant"; then
    BUNDLED_MODEL_OK=1
    break
  fi
done

if [[ $BUNDLED_MODEL_OK -eq 0 && "${BUNDLE_FALLBACK_GIT:-1}" == "1" ]]; then
  if command -v git >/dev/null 2>&1 && command -v git-lfs >/dev/null 2>&1; then
    TMP_M="$(mktemp -d)"/whisper-small-mlx
    echo "[i] No local models found; cloning mlx-community/whisper-small-mlx (shallow)…"
    if git clone --depth 1 https://huggingface.co/mlx-community/whisper-small-mlx "$TMP_M"; then
      (cd "$TMP_M" && git lfs install --local && git lfs pull || true)
      rsync -a "$TMP_M/" "$MODELS_DST/whisper-small-mlx/"
      echo "$MODELS_DST/whisper-small-mlx" >"$MODELS_DST/.last_whisper_variant"
      BUNDLED_MODEL_OK=1
    else
      echo "[!] Failed to clone fallback Whisper model."
    fi
  else
    echo "[!] No bundled model and git/git-lfs not available for fallback."
  fi
fi

if [[ $BUNDLED_MODEL_OK -eq 0 ]]; then
  echo "[!] Shipping without a bundled Whisper model. First launch will prompt to download one."
fi

echo "[✓] Wrapper app built: $APP_DIR"
