#!/bin/bash
# Build CodeScribe .app bundle + DMG with optional codesign + notarization

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="${CODESCRIBE_APP_NAME:-CodeScribe}"
DISPLAY_NAME="${CODESCRIBE_DISPLAY_NAME:-$APP_NAME}"
BUNDLE_ID="${CODESCRIBE_BUNDLE_ID:-com.codescribe.app}"
MIN_MACOS="${CODESCRIBE_MIN_MACOS:-}"
LSUIELEMENT="${CODESCRIBE_LSUIELEMENT:-true}"
ENTITLEMENTS="${CODESCRIBE_ENTITLEMENTS:-$ROOT_DIR/scripts/entitlements.plist}"
IDENTITY="${CODESCRIBE_CODESIGN_IDENTITY:-}"
NOTARY_PROFILE="${NOTARY_PROFILE:-VSNotary}"

VERSION=$(awk -F '"' '/^version[[:space:]]*=/{print $2; exit}' "$ROOT_DIR/Cargo.toml" 2>/dev/null || echo "0.0.0")
APP_PATH="$ROOT_DIR/bundle/${APP_NAME}.app"

SIGN=0
NOTARIZE=0
NO_EMBED=0
EMBED_WHISPER=0
DMG_SUFFIX=""

usage() {
  cat <<EOF
Usage: $0 [options]

Options:
  --sign              Codesign the .app (requires Developer ID)
  --notarize          Notarize the DMG (requires NOTARY_PROFILE)
  --identity <name>   Override codesign identity
  --entitlements <p>  Entitlements plist path (default: $ENTITLEMENTS)
  --embed-whisper     Embed the Whisper model in the app bundle
  --dmg-suffix <s>    Append suffix before .dmg (for example: _full)
  --no-embed          Disable all model embedding (CODESCRIBE_NO_EMBED=1)
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --sign) SIGN=1; shift 1;;
    --notarize) NOTARIZE=1; shift 1;;
    --identity) IDENTITY="$2"; shift 2;;
    --entitlements) ENTITLEMENTS="$2"; shift 2;;
    --embed-whisper) EMBED_WHISPER=1; shift 1;;
    --dmg-suffix) DMG_SUFFIX="$2"; shift 2;;
    --no-embed) NO_EMBED=1; shift 1;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown arg: $1" >&2; usage; exit 1;;
  esac
done

if [[ "$NO_EMBED" -eq 1 && "$EMBED_WHISPER" -eq 1 ]]; then
  echo "ERROR: --no-embed and --embed-whisper cannot be used together" >&2
  exit 1
fi

DMG_NAME="CodeScribe_${VERSION}${DMG_SUFFIX}.dmg"
DMG_PATH="$ROOT_DIR/$DMG_NAME"

BUILD_ENV=(env)
# All `-u` (unset) flags MUST precede any name=value assignments: BSD/macOS
# `env` stops parsing options at the first assignment (unlike GNU env), so an
# interleaved `-u` would be treated as the utility name (env: -u: No such file).
BUILD_ENV+=(-u CODESCRIBE_EMBED_TTS)
if [[ "$NO_EMBED" -eq 1 ]]; then
  BUILD_ENV+=(-u CODESCRIBE_EMBED_WHISPER CODESCRIBE_NO_EMBED=1)
else
  BUILD_ENV+=(-u CODESCRIBE_NO_EMBED)
  if [[ "$EMBED_WHISPER" -eq 1 ]]; then
    BUILD_ENV+=(CODESCRIBE_EMBED_WHISPER=1)
  else
    BUILD_ENV+=(-u CODESCRIBE_EMBED_WHISPER)
  fi
fi

echo "=== Build DMG ==="
echo "App: $APP_NAME"
echo "Bundle ID: $BUNDLE_ID"
echo "Version: $VERSION"
if [[ "$NO_EMBED" -eq 1 ]]; then
  echo "Models: runtime assets only (CODESCRIBE_NO_EMBED=1)"
elif [[ "$EMBED_WHISPER" -eq 1 ]]; then
  echo "Models: embedded Silero + embedder + Whisper"
else
  echo "Models: embedded Silero + embedder; Whisper resolves from cache/download"
fi
echo "DMG: $DMG_PATH"

(
  cd "$ROOT_DIR"
  "${BUILD_ENV[@]}" \
    CODESCRIBE_APP_NAME="$APP_NAME" \
    CODESCRIBE_DISPLAY_NAME="$DISPLAY_NAME" \
    CODESCRIBE_BUNDLE_ID="$BUNDLE_ID" \
    CODESCRIBE_MIN_MACOS="$MIN_MACOS" \
    CODESCRIBE_LSUIELEMENT="$LSUIELEMENT" \
    make bundle
)

if [[ ! -d "$APP_PATH" ]]; then
  echo "ERROR: App bundle not found at $APP_PATH" >&2
  exit 1
fi

if [[ "$SIGN" -eq 1 ]]; then
  if [[ -z "$IDENTITY" || "$IDENTITY" == "-" ]]; then
    echo "ERROR: --sign requires CODESCRIBE_CODESIGN_IDENTITY or --identity" >&2
    exit 1
  fi
  if [[ ! -f "$ENTITLEMENTS" ]]; then
    echo "ERROR: Entitlements file not found: $ENTITLEMENTS" >&2
    exit 1
  fi
  echo "Codesigning .app with identity: $IDENTITY"
  codesign --deep --force --options runtime --entitlements "$ENTITLEMENTS" --sign "$IDENTITY" "$APP_PATH"
  codesign --verify --deep --strict --verbose=2 "$APP_PATH" >/dev/null
fi

echo "Creating DMG..."
rm -f "$DMG_PATH"
TMP_DMG="$(mktemp -d)/dmg"
mkdir -p "$TMP_DMG"
cp -R "$APP_PATH" "$TMP_DMG/$APP_NAME.app"
ln -s /Applications "$TMP_DMG/Applications"
hdiutil create -volname "$DISPLAY_NAME" -srcfolder "$TMP_DMG" -ov -format UDZO "$DMG_PATH"

if [[ "$SIGN" -eq 1 ]]; then
  echo "Codesigning DMG..."
  codesign --force --sign "$IDENTITY" "$DMG_PATH" || true
fi

echo "DMG ready: $DMG_PATH"

if [[ "$NOTARIZE" -eq 1 ]]; then
  echo "Notarizing DMG with profile: $NOTARY_PROFILE"
  NOTARY_PROFILE="$NOTARY_PROFILE" "$ROOT_DIR/scripts/notarize.sh" "$DMG_PATH"
fi
