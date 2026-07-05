#!/usr/bin/env bash
#
# Build the CodeScribe SwiftUI app from the Rust `codescribe-ffi` bridge.
#
# This is the single source of truth for the SwiftUI build pipeline. Before it
# existed the steps below lived only in tribal memory / a reviewer's shell
# history, and a clean checkout could not produce a runnable app (the generated
# UniFFI bindings, Info.plist and .xcodeproj are all gitignored).
#
# Pipeline (each stage is deterministic and rerunnable):
#   1. cargo build -p codescribe-ffi   -> libcodescribe_ffi.dylib + uniffi-bindgen
#   2. install_name_tool -id @rpath/... -> make the dylib relocatable
#   3. uniffi-bindgen generate          -> Swift bindings into macos/Codescribe/Bridge
#   4. xcodegen generate                -> macos/Codescribe.xcodeproj + Info.plist
#   5. xcodebuild                        -> Codescribe.app
#   6. embed runtime artifacts so the bundle is self-contained
#   7. sign with a stable identifier so macOS TCC grants survive rebuilds
#
# Usage:
#   scripts/build-app.sh [debug|release]
#
# Env toggles:
#   SKIP_XCODEBUILD=1   stop after xcodegen (verifies stages 1-4 without Xcode)
#   CODE_SIGNING_ALLOWED=YES|NO   passed through to xcodebuild (default NO)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PROFILE="${1:-debug}"
case "$PROFILE" in
  debug)   CONFIG="Debug";   CARGO_FLAGS=();          TARGET_DIR="target/debug" ;;
  release) CONFIG="Release"; CARGO_FLAGS=(--release); TARGET_DIR="target/release" ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 2 ;;
esac

# ── Preflight: a clean checkout on a fresh Mac otherwise dies deep in the
# pipeline with a cryptic "command not found". Fail early, actionably.
require() {
  command -v "$1" >/dev/null 2>&1 || { echo "error: '$1' not found — $2" >&2; exit 1; }
}
require cargo    "install the Rust toolchain: https://rustup.rs"
require xcodegen "the app's .xcodeproj is generated, not committed: brew install xcodegen"
if [ "${SKIP_XCODEBUILD:-0}" != "1" ]; then
  require xcodebuild "install Xcode (App Store), then: sudo xcodebuild -runFirstLaunch"
  require swiftc "install Xcode command line tools: xcode-select --install"
fi

SCHEME="Codescribe"
BRIDGE_DIR="macos/Codescribe/Bridge"
DYLIB="$TARGET_DIR/libcodescribe_ffi.dylib"
BINDGEN="$TARGET_DIR/uniffi-bindgen"
STT_BRIDGE_SRC="core/stt/apple_stt/codescribe-stt-bridge.swift"
STT_BRIDGE_BIN="$TARGET_DIR/codescribe-stt-bridge"

echo "==> [1/7] Building codescribe-ffi ($PROFILE)"
cargo build -p codescribe-ffi "${CARGO_FLAGS[@]}"

echo "==> [2/7] Rewriting dylib install_name to @rpath (relocatable bundle)"
install_name_tool -id @rpath/libcodescribe_ffi.dylib "$DYLIB"

echo "==> [3/7] Generating Swift bindings via uniffi-bindgen"
mkdir -p "$BRIDGE_DIR"
"$BINDGEN" generate --library "$DYLIB" --language swift --out-dir "$BRIDGE_DIR"

echo "==> [4/7] Generating Xcode project (xcodegen)"
( cd macos && xcodegen generate )

if [ "${SKIP_XCODEBUILD:-0}" = "1" ]; then
  echo "==> SKIP_XCODEBUILD=1 — stopping after xcodegen (stages 1-4 verified)."
  exit 0
fi

echo "==> [5/7] Building app (xcodebuild, $CONFIG)"
DERIVED="$REPO_ROOT/macos/build"
xcodebuild -project macos/Codescribe.xcodeproj \
  -scheme "$SCHEME" -configuration "$CONFIG" \
  -derivedDataPath "$DERIVED" \
  CODE_SIGNING_ALLOWED="${CODE_SIGNING_ALLOWED:-NO}" \
  build

APP="$DERIVED/Build/Products/$CONFIG/$SCHEME.app"
echo "==> [6/7] Embedding runtime artifacts into $SCHEME.app"
FRAMEWORKS="$APP/Contents/Frameworks"
MACOS_DIR="$APP/Contents/MacOS"
mkdir -p "$FRAMEWORKS" "$MACOS_DIR"
cp "$DYLIB" "$FRAMEWORKS/"
swiftc -O -o "$STT_BRIDGE_BIN" "$STT_BRIDGE_SRC"
cp "$STT_BRIDGE_BIN" "$MACOS_DIR/"
chmod 755 "$MACOS_DIR/codescribe-stt-bridge"

# Ad-hoc sign the finished bundle with a STABLE identifier so macOS TCC
# (Accessibility / Input Monitoring) keeps its grant across rebuilds instead of
# re-prompting every time an unsigned binary's cdhash changes — the same
# identifier make install-app uses. `--deep` also covers the just-embedded dylib.
BUNDLE_ID="${CODESCRIBE_BUNDLE_ID:-com.vetcoders.codescribe}"
# Prefer a REAL signing identity (Developer ID / Apple Development). Its designated
# requirement is certificate-based, so a TCC grant (Accessibility / Input
# Monitoring) survives rebuilds. Ad-hoc (`--sign -`) is cdhash-based, so the grant
# dies on every rebuild — fall back to it only when no real identity exists.
SIGN_ID="${CODESCRIBE_CODESIGN_IDENTITY:-}"
if [ -z "$SIGN_ID" ] || [ "$SIGN_ID" = "-" ]; then
  SIGN_ID="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)"
  [ -z "$SIGN_ID" ] && SIGN_ID="$(security find-identity -v -p codesigning 2>/dev/null | sed -n 's/.*"\(Apple Development: [^"]*\)".*/\1/p' | head -1)"
fi
if [ -n "$SIGN_ID" ]; then
  echo "==> [7/7] Signing $SCHEME.app with stable identity: $SIGN_ID"
  codesign --force --deep --sign "$SIGN_ID" --identifier "$BUNDLE_ID" "$APP"
else
  echo "==> [7/7] Ad-hoc signing $SCHEME.app (no stable identity — TCC re-grants per build)"
  codesign --force --deep --sign - --identifier "$BUNDLE_ID" "$APP"
fi

echo "==> App built: $APP"
echo "    (portability: dylib is @rpath-relative and embedded; project.yml adds"
echo "     @executable_path/../Frameworks to the app runpath; Apple STT bridge"
echo "     is bundled beside the app executable in Contents/MacOS.)"
