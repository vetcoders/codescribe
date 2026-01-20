#!/bin/bash
# Build CodeScribe CLI release bundle
# Created by M&K (c)2026 VetCoders

set -e

echo "=== Building CodeScribe Release ==="

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
