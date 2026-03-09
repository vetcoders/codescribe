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

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Configuration
DEFAULT_REPO="LibraxisAI/whisper-large-v3-turbo-mlx-q8"
MODEL_REPO="${CODESCRIBE_EMBED_MODEL:-$DEFAULT_REPO}"

# If CODESCRIBE_EMBED_MODEL points to a local path, skip download.
if [[ -n "${CODESCRIBE_EMBED_MODEL:-}" ]] && [[ -d "${CODESCRIBE_EMBED_MODEL}" ]]; then
    if [[ -f "${CODESCRIBE_EMBED_MODEL}/config.json" ]]; then
        echo "✓ Whisper model found at ${CODESCRIBE_EMBED_MODEL} (local path). Skipping download."
        exit 0
    fi
fi

# If override isn't an HF repo, fall back to default repo.
if [[ "$MODEL_REPO" != */* ]]; then
    MODEL_REPO="$DEFAULT_REPO"
fi

MODEL_NAME="${MODEL_REPO##*/}"

echo "═══════════════════════════════════════════════════════════"
echo "  CodeScribe Model Download"
echo "═══════════════════════════════════════════════════════════"
echo "  Model:  ${MODEL_NAME}"
echo "  Source: https://huggingface.co/${MODEL_REPO}"
echo "───────────────────────────────────────────────────────────"

HF_BIN="$("$ROOT_DIR/scripts/ensure-hf-cli.sh")"

# Check authentication
echo ""
if [ -n "${HF_TOKEN:-}" ]; then
    echo "▶ Using HF_TOKEN from environment"
    export HF_TOKEN="$HF_TOKEN"
elif "$HF_BIN" auth whoami &>/dev/null; then
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

"$HF_BIN" download "$MODEL_REPO"

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Download Complete!"
echo "═══════════════════════════════════════════════════════════"
echo "  Location: HF cache (use: hf cache ls)"
echo ""
echo "  Model ready for use with CodeScribe."
echo "───────────────────────────────────────────────────────────"
