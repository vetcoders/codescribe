#!/bin/bash
# Codescribe Model Download Script
# Composes a complete, runtime-verified Whisper large-v3-turbo fp16 directory.
#
# Prerequisites:
#   - HF_TOKEN environment variable (for gated models)
#   - hf CLI installed: pip install huggingface_hub[cli]
#
# Usage:
#   HF_TOKEN=hf_xxx ./scripts/download-model.sh
#   ./scripts/download-model.sh  # Uses cached token from `hf auth login`
#
# Created by Vetcoders (c)2026

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

sha256_file() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        echo "ERROR: need shasum or sha256sum to verify model assets" >&2
        return 1
    fi
}

atomic_copy() {
    local source="$1"
    local destination="$2"
    local partial="${destination}.partial"
    cp -fL "$source" "$partial"
    mv -f "$partial" "$destination"
}

# Configuration
DEFAULT_REPO="mlx-community/whisper-large-v3-turbo"
TOKENIZER_REPO="openai/whisper-large-v3-turbo"
MEL_FILTERS_URL="https://raw.githubusercontent.com/openai/whisper/5f86d1d86363843179951550570367b37c5d6f78/whisper/assets/mel_filters.npz"
MEL_FILTERS_SHA256="7450ae70723a5ef9d341e3cee628c7cb0177f36ce42c44b7ed2bf3325f0f6d4c"
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
echo "  Codescribe Model Download"
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
    if [[ "${CI:-}" == "true" || ! -t 0 ]]; then
        # Non-interactive (CI or no TTY): never block on a prompt.
        # The default model is public and downloads without auth; a gated
        # model needs HF_TOKEN, in which case the download below fails clearly.
        echo "  Non-interactive mode: proceeding without auth."
        echo "  If the model is gated, set HF_TOKEN=hf_xxx and re-run."
    else
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
fi

# Download model
echo ""
echo "▶ Downloading model (HF cache)..."
echo "  This may take a few minutes..."
echo ""

MODEL_SNAPSHOT=$("$HF_BIN" download "$MODEL_REPO" --quiet)

# The default conversion ships only config + fp16 weights. Compose one
# self-contained product directory using the matching official OpenAI
# tokenizer and the pinned OpenAI mel filterbank. Q8 is not a fallback or an
# asset source anywhere in this path.
if [[ "$MODEL_REPO" == "$DEFAULT_REPO" ]]; then
    echo ""
    echo "▶ Composing verified fp16 runtime directory..."
    TOKENIZER_PATH=$("$HF_BIN" download "$TOKENIZER_REPO" tokenizer.json --quiet)
    MODEL_DEST="${CODESCRIBE_MODELS_DIR:-$HOME/.codescribe/models}/whisper-large-v3-turbo"
    mkdir -p "$MODEL_DEST"
    atomic_copy "$MODEL_SNAPSHOT/config.json" "$MODEL_DEST/config.json"
    if [[ -f "$MODEL_SNAPSHOT/weights.safetensors" ]]; then
        atomic_copy "$MODEL_SNAPSHOT/weights.safetensors" "$MODEL_DEST/weights.safetensors"
    elif [[ -f "$MODEL_SNAPSHOT/model.safetensors" ]]; then
        atomic_copy "$MODEL_SNAPSHOT/model.safetensors" "$MODEL_DEST/model.safetensors"
    else
        echo "ERROR: fp16 snapshot has no safetensors weights: $MODEL_SNAPSHOT" >&2
        exit 1
    fi
    atomic_copy "$TOKENIZER_PATH" "$MODEL_DEST/tokenizer.json"
    curl -fsSL "$MEL_FILTERS_URL" -o "$MODEL_DEST/mel_filters.npz.partial"
    ACTUAL_MEL_SHA=$(sha256_file "$MODEL_DEST/mel_filters.npz.partial")
    if [[ "$ACTUAL_MEL_SHA" != "$MEL_FILTERS_SHA256" ]]; then
        rm -f "$MODEL_DEST/mel_filters.npz.partial"
        echo "ERROR: mel_filters.npz checksum mismatch" >&2
        exit 1
    fi
    mv "$MODEL_DEST/mel_filters.npz.partial" "$MODEL_DEST/mel_filters.npz"
    echo "  Runtime directory: $MODEL_DEST"
fi

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Download Complete!"
echo "═══════════════════════════════════════════════════════════"
echo "  Source cache: $MODEL_SNAPSHOT"
echo ""
echo "  Model ready for use with Codescribe."
echo "───────────────────────────────────────────────────────────"
