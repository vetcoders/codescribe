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
WHISPER_VALIDATOR="$ROOT_DIR/scripts/validate-whisper-model.sh"
TILDE_PREFIX="$(printf '\176/')"
EMBED_MODEL_VALUE="${CODESCRIBE_EMBED_MODEL:-}"
if [[ "$EMBED_MODEL_VALUE" == "$TILDE_PREFIX"* ]]; then
    EMBED_MODEL_VALUE="$HOME/${EMBED_MODEL_VALUE:2}"
fi
MODELS_DIR_VALUE="${CODESCRIBE_MODELS_DIR:-$HOME/.codescribe/models}"
if [[ "$MODELS_DIR_VALUE" == "$TILDE_PREFIX"* ]]; then
    MODELS_DIR_VALUE="$HOME/${MODELS_DIR_VALUE:2}"
fi

is_hf_repo_id() {
    [[ "$1" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]]
}

looks_like_local_path() {
    [[ "$1" == /* || "$1" == ./* || "$1" == ../* ]]
}

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
MODEL_REPO="${EMBED_MODEL_VALUE:-$DEFAULT_REPO}"

# If CODESCRIBE_EMBED_MODEL points to a local path, skip download.
if [[ -n "$EMBED_MODEL_VALUE" ]] && [[ -d "$EMBED_MODEL_VALUE" ]]; then
    if "$WHISPER_VALIDATOR" "$EMBED_MODEL_VALUE"; then
        echo "✓ Whisper model found at $EMBED_MODEL_VALUE (local path). Skipping download."
        exit 0
    fi
    echo "ERROR: CODESCRIBE_EMBED_MODEL is not a valid Whisper bundle: $EMBED_MODEL_VALUE" >&2
    exit 1
fi

# An explicit path must never be passed to `hf download` as a repository id.
if [[ -n "$EMBED_MODEL_VALUE" ]] && looks_like_local_path "$MODEL_REPO"; then
    echo "ERROR: CODESCRIBE_EMBED_MODEL local path does not exist: $MODEL_REPO" >&2
    exit 1
fi

# A plain model alias keeps the historical default; only owner/repo selects HF.
if ! is_hf_repo_id "$MODEL_REPO"; then
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
if [[ -z "$MODEL_SNAPSHOT" || "$MODEL_SNAPSHOT" == *$'\n'* || ! -d "$MODEL_SNAPSHOT" ]]; then
    echo "ERROR: hf download did not return one snapshot directory for $MODEL_REPO" >&2
    exit 1
fi

# The default conversion ships only config + fp16 weights. Compose one
# self-contained product directory using the matching official OpenAI
# tokenizer and the pinned OpenAI mel filterbank. Q8 is not a fallback or an
# asset source anywhere in this path.
if [[ "$MODEL_REPO" == "$DEFAULT_REPO" ]]; then
    echo ""
    echo "▶ Composing verified fp16 runtime directory..."
    TOKENIZER_PATH=$("$HF_BIN" download "$TOKENIZER_REPO" tokenizer.json --quiet)
    MODEL_DEST="$MODELS_DIR_VALUE/whisper-large-v3-turbo"
    MODEL_STAGE=$(mktemp -d "${TMPDIR:-/tmp}/codescribe-whisper-model.XXXXXX")
    cleanup_model_stage() {
        rm -f \
            "$MODEL_STAGE/config.json" \
            "$MODEL_STAGE/tokenizer.json" \
            "$MODEL_STAGE/mel_filters.npz" \
            "$MODEL_STAGE/mel_filters.npz.partial" \
            "$MODEL_STAGE/weights.safetensors" \
            "$MODEL_STAGE/model.safetensors"
        rmdir "$MODEL_STAGE" 2>/dev/null || true
    }
    trap cleanup_model_stage EXIT

    atomic_copy "$MODEL_SNAPSHOT/config.json" "$MODEL_STAGE/config.json"
    if [[ -f "$MODEL_SNAPSHOT/weights.safetensors" ]]; then
        atomic_copy "$MODEL_SNAPSHOT/weights.safetensors" "$MODEL_STAGE/weights.safetensors"
    elif [[ -f "$MODEL_SNAPSHOT/model.safetensors" ]]; then
        atomic_copy "$MODEL_SNAPSHOT/model.safetensors" "$MODEL_STAGE/model.safetensors"
    else
        echo "ERROR: fp16 snapshot has no safetensors weights: $MODEL_SNAPSHOT" >&2
        exit 1
    fi
    atomic_copy "$TOKENIZER_PATH" "$MODEL_STAGE/tokenizer.json"
    curl -fsSL "$MEL_FILTERS_URL" -o "$MODEL_STAGE/mel_filters.npz.partial"
    ACTUAL_MEL_SHA=$(sha256_file "$MODEL_STAGE/mel_filters.npz.partial")
    if [[ "$ACTUAL_MEL_SHA" != "$MEL_FILTERS_SHA256" ]]; then
        echo "ERROR: mel_filters.npz checksum mismatch" >&2
        exit 1
    fi
    mv "$MODEL_STAGE/mel_filters.npz.partial" "$MODEL_STAGE/mel_filters.npz"

    # Do not touch a working installation until every cached/downloaded
    # replacement passes the exact runtime-owned bundle contract.
    "$WHISPER_VALIDATOR" "$MODEL_STAGE"

    mkdir -p "$MODEL_DEST"
    atomic_copy "$MODEL_STAGE/config.json" "$MODEL_DEST/config.json"
    atomic_copy "$MODEL_STAGE/tokenizer.json" "$MODEL_DEST/tokenizer.json"
    atomic_copy "$MODEL_STAGE/mel_filters.npz" "$MODEL_DEST/mel_filters.npz"
    if [[ -f "$MODEL_STAGE/weights.safetensors" ]]; then
        atomic_copy "$MODEL_STAGE/weights.safetensors" "$MODEL_DEST/weights.safetensors"
        rm -f "$MODEL_DEST/model.safetensors"
    else
        atomic_copy "$MODEL_STAGE/model.safetensors" "$MODEL_DEST/model.safetensors"
        rm -f "$MODEL_DEST/weights.safetensors"
    fi
    echo "  Runtime directory: $MODEL_DEST"
    "$WHISPER_VALIDATOR" "$MODEL_DEST"
    cleanup_model_stage
    trap - EXIT
else
    "$WHISPER_VALIDATOR" "$MODEL_SNAPSHOT"
fi

echo ""
echo "═══════════════════════════════════════════════════════════"
echo "  Download Complete!"
echo "═══════════════════════════════════════════════════════════"
echo "  Source cache: $MODEL_SNAPSHOT"
echo ""
echo "  Model ready for use with Codescribe."
echo "───────────────────────────────────────────────────────────"
