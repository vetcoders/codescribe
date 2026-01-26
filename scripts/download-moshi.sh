#!/bin/bash
# Download Moshi Conversational AI models from HuggingFace
#
# Usage:
#   ./scripts/download-moshi.sh           # Download both voices
#   ./scripts/download-moshi.sh moshiko   # Download only male voice
#   ./scripts/download-moshi.sh moshika   # Download only female voice
#
# Downloads to: ~/.codescribe/models/moshiko-q8/ and ~/.codescribe/models/moshika-q8/
#
# Note: Mimi codec is shared with CSM - run download-csm.sh if not present.
#
# Created by M&K (c)2026 VetCoders

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# All models go to ~/.codescribe/models/
MODELS_DIR="${HOME}/.codescribe/models"

# Parse arguments
DOWNLOAD_MOSHIKO=true
DOWNLOAD_MOSHIKA=true

if [ "${1:-}" = "moshiko" ]; then
    DOWNLOAD_MOSHIKA=false
elif [ "${1:-}" = "moshika" ]; then
    DOWNLOAD_MOSHIKO=false
fi

echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  Downloading Moshi Conversational AI Models               ║"
echo "╚═══════════════════════════════════════════════════════════╝"
echo ""
echo "Target directory: $MODELS_DIR"
echo ""

# Check for huggingface-cli
if ! command -v huggingface-cli &> /dev/null; then
    echo "⚠️  huggingface-cli not found. Installing..."
    pip install -U huggingface_hub
    echo ""
    echo "Please run this script again after installation."
    exit 1
fi

# Create models directory
mkdir -p "$MODELS_DIR"

# Download Moshiko (male voice)
if [ "$DOWNLOAD_MOSHIKO" = true ]; then
    MOSHIKO_DIR="$MODELS_DIR/moshiko-q8"

    if [ -d "$MOSHIKO_DIR" ] && [ -f "$MOSHIKO_DIR/model.safetensors" ]; then
        MODEL_SIZE=$(du -sh "$MOSHIKO_DIR" | cut -f1)
        echo "✓ Moshiko already downloaded: $MOSHIKO_DIR ($MODEL_SIZE)"
        echo "  To re-download, remove the directory first:"
        echo "    rm -rf $MOSHIKO_DIR"
        echo ""
    else
        echo "📦 Downloading Moshiko (male voice) ~8GB..."
        echo "   Source: kyutai/moshiko-candle-q8"
        echo ""

        huggingface-cli download kyutai/moshiko-candle-q8 \
            --include "*.safetensors" "*.json" \
            --local-dir "$MOSHIKO_DIR"

        echo ""
        echo "✅ Moshiko downloaded successfully!"
        echo ""
    fi
fi

# Download Moshika (female voice)
if [ "$DOWNLOAD_MOSHIKA" = true ]; then
    MOSHIKA_DIR="$MODELS_DIR/moshika-q8"

    if [ -d "$MOSHIKA_DIR" ] && [ -f "$MOSHIKA_DIR/model.safetensors" ]; then
        MODEL_SIZE=$(du -sh "$MOSHIKA_DIR" | cut -f1)
        echo "✓ Moshika already downloaded: $MOSHIKA_DIR ($MODEL_SIZE)"
        echo "  To re-download, remove the directory first:"
        echo "    rm -rf $MOSHIKA_DIR"
        echo ""
    else
        echo "📦 Downloading Moshika (female voice) ~8GB..."
        echo "   Source: kyutai/moshika-candle-q8"
        echo ""

        huggingface-cli download kyutai/moshika-candle-q8 \
            --include "*.safetensors" "*.json" \
            --local-dir "$MOSHIKA_DIR"

        echo ""
        echo "✅ Moshika downloaded successfully!"
        echo ""
    fi
fi

# Check for Mimi codec (shared with CSM)
MIMI_FILE="${HOME}/.codescribe/models/csm-1b/mimi.safetensors"
if [ ! -f "$MIMI_FILE" ]; then
    echo "⚠️  Mimi codec not found at: $MIMI_FILE"
    echo "   Moshi requires Mimi for audio encoding/decoding."
    echo "   Run: ./scripts/download-csm.sh"
    echo ""
fi

# Verify downloads
echo "═══════════════════════════════════════════════════════════"
echo "  Download Summary"
echo "═══════════════════════════════════════════════════════════"

if [ "$DOWNLOAD_MOSHIKO" = true ] && [ -d "$MODELS_DIR/moshiko-q8" ]; then
    SIZE=$(du -sh "$MODELS_DIR/moshiko-q8" | cut -f1)
    echo "  ✓ Moshiko (male):   $MODELS_DIR/moshiko-q8 ($SIZE)"
fi

if [ "$DOWNLOAD_MOSHIKA" = true ] && [ -d "$MODELS_DIR/moshika-q8" ]; then
    SIZE=$(du -sh "$MODELS_DIR/moshika-q8" | cut -f1)
    echo "  ✓ Moshika (female): $MODELS_DIR/moshika-q8 ($SIZE)"
fi

if [ -f "$MIMI_FILE" ]; then
    SIZE=$(du -h "$MIMI_FILE" | cut -f1)
    echo "  ✓ Mimi codec:       $MIMI_FILE ($SIZE)"
else
    echo "  ✗ Mimi codec:       Not found (run download-csm.sh)"
fi

echo ""
echo "╔═══════════════════════════════════════════════════════════╗"
echo "║  Next steps:                                              ║"
echo "║                                                           ║"
echo "║  Test Moshi conversation engine:                          ║"
echo "║    cargo run --example voice_chat_demo                    ║"
echo "║                                                           ║"
echo "║  Select voice in code:                                    ║"
echo "║    let config = MoshiConfig::moshiko(); // male           ║"
echo "║    let config = MoshiConfig::moshika(); // female         ║"
echo "╚═══════════════════════════════════════════════════════════╝"
