#!/bin/bash
# CodeScribe Silero VAD Download Script
# Downloads silero-vad ONNX model from HuggingFace
#
# Usage:
#   ./scripts/download-silero-vad.sh
#
# Created by M&K (c)2026 VetCoders

set -euo pipefail

# Configuration
MODEL_REPO="snakers4/silero-vad"

echo "═══════════════════════════════════════════════════════════"
echo "  CodeScribe Silero VAD Download"
echo "═══════════════════════════════════════════════════════════"
echo "  Model:  ${MODEL_REPO}"
echo "  Source: https://huggingface.co/${MODEL_REPO}"
echo "───────────────────────────────────────────────────────────"

# Check for HuggingFace CLI
if ! command -v hf &> /dev/null; then
    echo ""
    echo "▶ Installing hf CLI..."
    pip install -q huggingface_hub[cli]
fi

# Download model (public, no auth needed)
echo ""
echo "▶ Downloading model (HF cache)..."
echo "  This should be quick (~2MB)..."
echo ""

hf download "$MODEL_REPO" \
  --include "silero_vad.onnx"

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Download Complete!"
echo "═══════════════════════════════════════════════════════════"
echo "  Location: HF cache (use: hf cache ls)"
echo ""
echo "  Model ready for use with CodeScribe VAD."
echo "───────────────────────────────────────────────────────────"
