#!/bin/zsh
# Build CodeScribe.app bundle with Rust frontend + Python backend
#
# Architecture:
#   - Rust binary (codescribe) as the main executable
#   - Rust binary starts Python backend (whisper_server.py) via uv
#   - Python backend uses FastAPI + MLX Whisper for transcription
#
# Bundle structure:
#   CodeScribe.app/
#   ├── Contents/
#   │   ├── Info.plist
#   │   ├── MacOS/
#   │   │   ├── CodeScribe (shell wrapper)
#   │   │   └── CodeScribe.bin (Rust binary)
#   │   └── Resources/
#   │       ├── AppIcon.icns
#   │       ├── assets/ (vocabulary JSONL files)
#   │       ├── python/ (whisper_server.py + codescribe package + pyproject.toml)
#   │       └── Models/ (optional bundled Whisper models)

set -euo pipefail

ROOT_DIR="$(cd -- "$(dirname "$0")/../.." && pwd)"
DIST_DIR="$ROOT_DIR/packaging/dist"
APP_DIR="$DIST_DIR/CodeScribe.app"

# Resolve version from Cargo.toml
VERSION=$(awk -F '"' '/^version[[:space:]]*=/{print $2; exit}' "$ROOT_DIR/Cargo.toml" 2>/dev/null || echo "0.0.0")

echo "[i] Building CodeScribe.app with Rust frontend at: $APP_DIR"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

# Create bundle subdirectories
ASSETS_DST="$APP_DIR/Contents/Resources/assets"
PYTHON_DST="$APP_DIR/Contents/Resources/python"
MODELS_DST="$APP_DIR/Contents/Resources/Models"
mkdir -p "$ASSETS_DST" "$PYTHON_DST" "$MODELS_DST"

bundle_whisper_model() {
  local variant="$1"
  local server=""
  for candidate in \
    "$ROOT_DIR/models/whisper-${variant}" \
    "$ROOT_DIR/models/${variant}"; do
    if [[ -d "$candidate" ]]; then
      server="$candidate"
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

# Priority: Q8 quantized models first (smaller, same quality)
BUNDLE_VARIANTS=(${BUNDLE_VARIANTS:-medium-mlx-q8 large-v3-turbo-mlx-q8 small-mlx-q8 medium large-v3-turbo small})

# Info.plist
cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>CodeScribe</string>
  <key>CFBundleDisplayName</key><string>CodeScribe</string>
  <key>CFBundleIdentifier</key><string>com.codescribe.app</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>CodeScribe</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSUIElement</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Needed to transcribe speech.</string>
  <key>NSAccessibilityUsageDescription</key><string>Needed to monitor hotkeys and paste results.</string>
  <key>NSInputMonitoringUsageDescription</key><string>Needed to detect keyboard shortcuts for recording.</string>
</dict>
</plist>
PLIST

# Build Rust binary first
echo "[i] Building Rust binary..."
cd "$ROOT_DIR"
cargo build --release -p codescribe
RUST_BIN="$ROOT_DIR/target/release/codescribe"

if [[ ! -f "$RUST_BIN" ]]; then
  echo "[!] ERROR: Rust binary not found at $RUST_BIN"
  exit 1
fi

# Copy Rust binary to app bundle
echo "[i] Copying Rust binary to app bundle..."
cp "$RUST_BIN" "$APP_DIR/Contents/MacOS/CodeScribe"
chmod +x "$APP_DIR/Contents/MacOS/CodeScribe"

# Note: The actual launcher is now the Rust binary itself (CodeScribe)
# But we need a wrapper script for Finder to properly set environment variables
# macOS .app bundles execute the CFBundleExecutable directly, but we can't set env vars in Info.plist
# So we create a shell wrapper as the executable and have it call the real Rust binary

# Rename the Rust binary to the actual executable
mv "$APP_DIR/Contents/MacOS/CodeScribe" "$APP_DIR/Contents/MacOS/CodeScribe.bin"

# Create launcher wrapper that sets up environment before running Rust binary
cat > "$APP_DIR/Contents/MacOS/CodeScribe" <<'LAUNCH'
#!/bin/zsh
set -euo pipefail
APP_DIR="$(cd -- "$(dirname "$0")/.." && pwd)"
LOG_DIR="$HOME/Library/Logs"
mkdir -p "$LOG_DIR"
LOG_FILE="$LOG_DIR/CodeScribe.app.log"
MODELS_DIR="$APP_DIR/Resources/Models"
ASSETS_DIR="$APP_DIR/Resources/assets"
PYTHON_DIR="$APP_DIR/Resources/python"

# Ensure uv and brew binaries are on PATH when launched from Finder
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

# Set paths for bundled resources
export CODESCRIBE_ASSETS_PATH="$ASSETS_DIR"
export CODESCRIBE_PYTHON_DIR="$PYTHON_DIR"

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

# Run the Rust binary (it will start Python backend automatically via uv)
cd "$APP_DIR/MacOS"
exec ./CodeScribe.bin >> "$LOG_FILE" 2>&1
LAUNCH
chmod +x "$APP_DIR/Contents/MacOS/CodeScribe"

# Build app icon from logo.png
ICON_SRC="$ROOT_DIR/assets/logo.png"
if [[ -f "$ICON_SRC" ]]; then
  echo "[i] Generating AppIcon.icns from assets/logo.png"
  ICONSET_DIR="$(mktemp -d)/AppIcon.iconset"
  mkdir -p "$ICONSET_DIR"
  for sz in 16 32 64 128 256 512; do
    /usr/bin/sips -z $sz $sz "$ICON_SRC" --out "$ICONSET_DIR/icon_${sz}x${sz}.png" >/dev/null 2>&1 || true
    /usr/bin/sips -z $((sz*2)) $((sz*2)) "$ICON_SRC" --out "$ICONSET_DIR/icon_${sz}x${sz}@2x.png" >/dev/null 2>&1 || true
  done
  /usr/bin/iconutil -c icns "$ICONSET_DIR" -o "$APP_DIR/Contents/Resources/AppIcon.icns" || true
else
  echo "[!] Warning: logo.png not found at $ICON_SRC"
fi

# Copy vocabulary assets
echo "[i] Copying vocabulary assets..."
if [[ -d "$ROOT_DIR/assets" ]]; then
  cp "$ROOT_DIR/assets/"*.jsonl "$ASSETS_DST/" 2>/dev/null || echo "[!] Warning: No JSONL files found in assets/"
else
  echo "[!] Warning: Assets directory not found"
fi

# Copy Python backend files (whisper_server + codescribe package)
echo "[i] Copying Python backend files..."
cp "$ROOT_DIR/whisper_server.py" "$PYTHON_DST/" || echo "[!] Warning: whisper_server.py not found"

# Copy the entire codescribe package
if [[ -d "$ROOT_DIR/src/codescribe" ]]; then
  echo "[i] Copying codescribe package..."
  mkdir -p "$PYTHON_DST/src"
  rsync -a "$ROOT_DIR/src/codescribe" "$PYTHON_DST/src/" --exclude '__pycache__' --exclude '*.pyc'
else
  echo "[!] Warning: codescribe package not found at $ROOT_DIR/src/codescribe"
fi

# Copy Python project files for dependency management
echo "[i] Copying Python project configuration..."
cp "$ROOT_DIR/pyproject.toml" "$PYTHON_DST/" 2>/dev/null || echo "[!] Warning: pyproject.toml not found"
cp "$ROOT_DIR/uv.lock" "$PYTHON_DST/" 2>/dev/null || true
cp "$ROOT_DIR/README.md" "$PYTHON_DST/" 2>/dev/null || true

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
