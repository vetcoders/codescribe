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

# Configuration
MODEL_REPO="LibraxisAI/whisper-large-v3-turbo-mlx-q8"
MODEL_NAME="whisper-large-v3-turbo-mlx-q8"

echo "═══════════════════════════════════════════════════════════"
echo "  CodeScribe Model Download"
echo "═══════════════════════════════════════════════════════════"
echo "  Model:  ${MODEL_NAME}"
echo "  Source: https://huggingface.co/${MODEL_REPO}"
echo "───────────────────────────────────────────────────────────"

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

# Download model
echo ""
echo "▶ Downloading model (HF cache)..."
echo "  This may take a few minutes..."
echo ""

hf download "$MODEL_REPO"

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Download Complete!"
echo "═══════════════════════════════════════════════════════════"
echo "  Location: HF cache (use: hf cache ls)"
echo ""
echo "  Model ready for use with CodeScribe."
echo "───────────────────────────────────────────────────────────"
