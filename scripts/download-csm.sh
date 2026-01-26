#!/bin/bash
# Download CSM-1B TTS model from HuggingFace
#
# Usage: ./scripts/download-csm.sh
#
# Downloads to HuggingFace cache (hf download)
#
# Created by M&K (c)2026 VetCoders

set -e

echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  Downloading CSM-1B Text-to-Speech Model                 ║"
echo "╚═══════════════════════════════════════════════════════════╝"
echo ""
echo "Target directory: HF cache (use: hf cache ls)"
echo ""

# Check for hf CLI
if command -v hf &> /dev/null; then
    echo "Using hf CLI..."

    # Download CSM model
    echo ""
    echo "📦 Downloading CSM-1B model (~1GB)..."
    hf download sesame/csm-1b \
        --include "*.safetensors" "*.json"

    # Download Mimi codec
    echo ""
    echo "📦 Downloading Mimi codec (~90MB)..."
    hf download kyutai/mimi \
        --include "model.safetensors" "config.json"

else
    echo "⚠️  hf CLI not found. Installing..."
    pip install -U huggingface_hub

    echo ""
    echo "Please run this script again after installation."
    exit 1
fi

echo ""
echo "✅ Download complete!"
echo ""
echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  Next steps:                                              ║"
echo "║                                                           ║"
echo "║  Development (path-based loading):                        ║"
echo "║    export CODESCRIBE_TTS_PATH=<local_dir>                 ║"
echo "║    cargo run                                              ║"
echo "║                                                           ║"
echo "║  Release (embedded model):                                ║"
echo "║    CODESCRIBE_EMBED_TTS=1 cargo build --release           ║"
echo "╚═══════════════════════════════════════════════════════════╝"
