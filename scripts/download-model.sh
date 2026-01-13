#!/bin/bash
# CodeScribe Model Download Script
# Downloads whisper-large-v3-turbo-mlx-q8 from HuggingFace
#
# Prerequisites:
#   - HF_TOKEN environment variable (for gated models)
#   - hf CLI installed: pip install huggingface_hub[cli]
#
# Usage:
#   HF_TOKEN=hf_xxx ./scripts/download-model.sh
#   ./scripts/download-model.sh  # Uses cached token from `hf auth login`
#
# Created by M&K (c)2026 VetCoders

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_DIR"

# Configuration
MODEL_REPO="LibraxisAI/whisper-large-v3-turbo-mlx-q8"
MODEL_NAME="whisper-large-v3-turbo-mlx-q8"
MODEL_DIR="models/${MODEL_NAME}"

echo "═══════════════════════════════════════════════════════════"
echo "  CodeScribe Model Download"
echo "═══════════════════════════════════════════════════════════"
echo "  Model:  ${MODEL_NAME}"
echo "  Source: https://huggingface.co/${MODEL_REPO}"
echo "───────────────────────────────────────────────────────────"

# Check if model already exists
if [ -d "$MODEL_DIR" ] && [ -f "${MODEL_DIR}/weights.safetensors" ]; then
    MODEL_SIZE=$(du -sh "$MODEL_DIR" | cut -f1)
    echo ""
    echo "  ✓ Model already downloaded: ${MODEL_DIR} (${MODEL_SIZE})"
    echo ""
    echo "  To re-download, remove the directory first:"
    echo "    rm -rf ${MODEL_DIR}"
    exit 0
fi

# Check for HuggingFace CLI
if ! command -v hf &> /dev/null; then
    echo ""
    echo "▶ Installing hf CLI..."
    pip install -q huggingface_hub[cli]
fi

# Check authentication
echo ""
if [ -n "${HF_TOKEN:-}" ]; then
    echo "▶ Using HF_TOKEN from environment"
    export HF_TOKEN="$HF_TOKEN"
elif hf auth whoami &>/dev/null; then
    echo "▶ Using cached HuggingFace credentials"
else
    echo "⚠ No HuggingFace authentication found"
    echo ""
    echo "  For gated models, you need to authenticate:"
    echo "    1. Create token at https://huggingface.co/settings/tokens"
    echo "    2. Run: hf auth login"
    echo "    3. Or set: export HF_TOKEN=hf_xxx"
    echo ""
    read -p "  Continue without auth? (y/n) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Create models directory
mkdir -p models

# Download model
echo ""
echo "▶ Downloading model (~900MB)..."
echo "  This may take a few minutes..."
echo ""

hf download "$MODEL_REPO" --local-dir "$MODEL_DIR"

# Verify download
echo ""
echo "▶ Verifying model files..."

REQUIRED_FILES=("config.json" "weights.safetensors" "tokenizer.json" "mel_filters.npz")
MISSING=()

for file in "${REQUIRED_FILES[@]}"; do
    if [ ! -f "${MODEL_DIR}/${file}" ]; then
        MISSING+=("$file")
    fi
done

if [ ${#MISSING[@]} -gt 0 ]; then
    echo "✗ Missing required files: ${MISSING[*]}"
    exit 1
fi

MODEL_SIZE=$(du -sh "$MODEL_DIR" | cut -f1)

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Download Complete!"
echo "═══════════════════════════════════════════════════════════"
echo "  Location: ${MODEL_DIR}"
echo "  Size:     ${MODEL_SIZE}"
echo ""
echo "  Files:"
for file in "${REQUIRED_FILES[@]}"; do
    SIZE=$(du -h "${MODEL_DIR}/${file}" | cut -f1)
    echo "    ✓ ${file} (${SIZE})"
done
echo ""
echo "  Model ready for use with CodeScribe."
echo "───────────────────────────────────────────────────────────"
