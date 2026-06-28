#!/bin/bash
# Download Silero VAD model from GitHub
#
# Usage: ./scripts/download-silero.sh
#
# Downloads to: ~/.codescribe/models/silero_vad.onnx
#
# Model: Silero VAD v5 (ONNX) from https://github.com/snakers4/silero-vad
#
# Created by Vetcoders (c)2026

set -e

# All models go to ~/.codescribe/models/
MODEL_DIR="${HOME}/.codescribe/models"
MODEL_FILE="$MODEL_DIR/silero_vad.onnx"

# Silero VAD v5 ONNX model URL
SILERO_URL="https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx"

echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  Downloading Silero VAD Model (v5)                        ║"
echo "╚═══════════════════════════════════════════════════════════╝"
echo ""
echo "Target: $MODEL_FILE"
echo "Source: $SILERO_URL"
echo ""

# Check if already exists
if [ -f "$MODEL_FILE" ]; then
    SIZE=$(du -h "$MODEL_FILE" | cut -f1)
    echo "✓ Silero VAD already downloaded: $MODEL_FILE ($SIZE)"
    echo "  To re-download, remove the file first:"
    echo "    rm $MODEL_FILE"
    echo ""
    exit 0
fi

# Create directory
mkdir -p "$MODEL_DIR"

# Download
echo "📦 Downloading Silero VAD model (~2MB)..."
if command -v curl &> /dev/null; then
    curl -L -o "$MODEL_FILE" "$SILERO_URL"
elif command -v wget &> /dev/null; then
    wget -O "$MODEL_FILE" "$SILERO_URL"
else
    echo "❌ Error: Neither curl nor wget found. Install one and try again."
    exit 1
fi

# Verify
if [ -f "$MODEL_FILE" ]; then
    SIZE=$(du -h "$MODEL_FILE" | cut -f1)
    echo ""
    echo "✅ Download complete!"
    echo "   File: $MODEL_FILE"
    echo "   Size: $SIZE"
else
    echo "❌ Download failed!"
    exit 1
fi

echo ""
echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  Silero VAD is ready!                                     ║"
echo "║                                                           ║"
echo "║  VAD will auto-initialize when recording starts.          ║"
echo "║  VAD defaults are hardcoded in core/vad/config.rs.        ║"
echo "╚═══════════════════════════════════════════════════════════╝"
