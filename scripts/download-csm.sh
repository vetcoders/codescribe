#!/bin/bash
# Download CSM-1B TTS model from HuggingFace
#
# Usage: ./scripts/download-csm.sh
#
# Downloads to: models/csm-1b/
#
# Created by M&K (c)2026 VetCoders

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
MODEL_DIR="$PROJECT_DIR/models/csm-1b"

echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  Downloading CSM-1B Text-to-Speech Model                 ║"
echo "╚═══════════════════════════════════════════════════════════╝"
echo ""
echo "Target directory: $MODEL_DIR"
echo ""

# Create directory
mkdir -p "$MODEL_DIR"

# Check for huggingface-cli
if command -v huggingface-cli &> /dev/null; then
    echo "Using huggingface-cli..."

    # Download CSM model
    echo ""
    echo "📦 Downloading CSM-1B model (~1GB)..."
    huggingface-cli download sesame/csm-1b \
        --include "*.safetensors" "*.json" \
        --local-dir "$MODEL_DIR"

    # Download Mimi codec
    echo ""
    echo "📦 Downloading Mimi codec (~90MB)..."
    huggingface-cli download kyutai/mimi \
        --include "model.safetensors" "config.json" \
        --local-dir "$MODEL_DIR/mimi_tmp"

    # Move Mimi files to main directory with proper names
    if [ -f "$MODEL_DIR/mimi_tmp/model.safetensors" ]; then
        mv "$MODEL_DIR/mimi_tmp/model.safetensors" "$MODEL_DIR/mimi.safetensors"
    fi
    if [ -f "$MODEL_DIR/mimi_tmp/config.json" ]; then
        mv "$MODEL_DIR/mimi_tmp/config.json" "$MODEL_DIR/mimi_config.json"
    fi
    rm -rf "$MODEL_DIR/mimi_tmp"

else
    echo "⚠️  huggingface-cli not found. Installing..."
    pip install -U huggingface_hub

    echo ""
    echo "Please run this script again after installation."
    exit 1
fi

echo ""
echo "✅ Download complete!"
echo ""
echo "Files in $MODEL_DIR:"
ls -lh "$MODEL_DIR"

echo ""
echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  Next steps:                                              ║"
echo "║                                                           ║"
echo "║  Development (path-based loading):                        ║"
echo "║    export CODESCRIBE_TTS_PATH=$MODEL_DIR                  ║"
echo "║    cargo run                                              ║"
echo "║                                                           ║"
echo "║  Release (embedded model):                                ║"
echo "║    CODESCRIBE_EMBED_TTS=1 cargo build --release           ║"
echo "╚═══════════════════════════════════════════════════════════╝"
