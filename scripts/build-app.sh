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
#   6. embed the dylib in Contents/Frameworks so the bundle is self-contained
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
fi

SCHEME="Codescribe"
BRIDGE_DIR="macos/Codescribe/Bridge"
DYLIB="$TARGET_DIR/libcodescribe_ffi.dylib"
BINDGEN="$TARGET_DIR/uniffi-bindgen"

echo "==> [1/6] Building codescribe-ffi ($PROFILE)"
cargo build -p codescribe-ffi "${CARGO_FLAGS[@]}"

echo "==> [2/6] Rewriting dylib install_name to @rpath (relocatable bundle)"
install_name_tool -id @rpath/libcodescribe_ffi.dylib "$DYLIB"

echo "==> [3/6] Generating Swift bindings via uniffi-bindgen"
mkdir -p "$BRIDGE_DIR"
"$BINDGEN" generate --library "$DYLIB" --language swift --out-dir "$BRIDGE_DIR"

echo "==> [4/6] Generating Xcode project (xcodegen)"
( cd macos && xcodegen generate )

if [ "${SKIP_XCODEBUILD:-0}" = "1" ]; then
  echo "==> SKIP_XCODEBUILD=1 — stopping after xcodegen (stages 1-4 verified)."
  exit 0
fi

echo "==> [5/6] Building app (xcodebuild, $CONFIG)"
DERIVED="$REPO_ROOT/macos/build"
xcodebuild -project macos/Codescribe.xcodeproj \
  -scheme "$SCHEME" -configuration "$CONFIG" \
  -derivedDataPath "$DERIVED" \
  CODE_SIGNING_ALLOWED="${CODE_SIGNING_ALLOWED:-NO}" \
  build

APP="$DERIVED/Build/Products/$CONFIG/$SCHEME.app"
echo "==> [6/6] Embedding dylib into $SCHEME.app/Contents/Frameworks"
FRAMEWORKS="$APP/Contents/Frameworks"
mkdir -p "$FRAMEWORKS"
cp "$DYLIB" "$FRAMEWORKS/"

echo "==> App built: $APP"
echo "    (portability: dylib is @rpath-relative and embedded; project.yml adds"
echo "     @executable_path/../Frameworks to the app runpath.)"
