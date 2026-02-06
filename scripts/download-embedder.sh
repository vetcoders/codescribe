#!/bin/bash
# CodeScribe Embedder Download Script
# Downloads paraphrase-multilingual-MiniLM-L12-v2 (or override) from HuggingFace
#
# Prerequisites:
#   - hf CLI installed: pip install huggingface_hub[cli]
#
# Usage:
#   HF_TOKEN=hf_xxx ./scripts/download-embedder.sh
#   CODESCRIBE_EMBEDDER_REPO=your/repo ./scripts/download-embedder.sh
#
# Created by M&K (c)2026 VetCoders

set -euo pipefail

MODEL_REPO="${CODESCRIBE_EMBEDDER_REPO:-sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2}"
MODEL_NAME="${MODEL_REPO##*/}"

echo "═══════════════════════════════════════════════════════════"
echo "  CodeScribe Embedder Download"
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

# Check authentication (if needed)
echo ""
if [ -n "${HF_TOKEN:-}" ]; then
    echo "▶ Using HF_TOKEN from environment"
    export HF_TOKEN="$HF_TOKEN"
elif hf auth whoami &>/dev/null; then
    echo "▶ Using cached HuggingFace credentials"
else
    echo "⚠ No HuggingFace authentication found"
    echo ""
    echo "  If the model is gated, you need to authenticate:"
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
echo "▶ Downloading embedder (HF cache)..."
echo "  This may take a few minutes..."
echo ""

hf download "$MODEL_REPO"

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Download Complete!"
echo "═══════════════════════════════════════════════════════════"
echo "  Location: HF cache (use: hf cache ls)"
echo ""
echo "  Embedder ready for use with CodeScribe."
echo "───────────────────────────────────────────────────────────"
