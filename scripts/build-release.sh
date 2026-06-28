#!/bin/bash
# Build Codescribe CLI release bundle
# Created by Vetcoders (c)2026

set -e

echo "=== Building Codescribe Release ==="

# Get target triple
TARGET=$(rustc -vV | grep host | cut -d' ' -f2)
echo "Target: $TARGET"

# 1. Build CLI (release)
echo ""
echo ">>> Building codescribe (CLI engine)..."
cargo build --release -p codescribe

# 2. Copy with target triple suffix for external wrappers
CLI_BIN="target/release/codescribe"
SIDECAR_BIN="target/release/codescribe-${TARGET}"

echo ">>> Creating sidecar: $SIDECAR_BIN"
cp "$CLI_BIN" "$SIDECAR_BIN"

echo ""
echo "=== Build Complete ==="
echo "CLI: target/release/codescribe"
echo "Sidecar: $SIDECAR_BIN"
