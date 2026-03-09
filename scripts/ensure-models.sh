#!/bin/bash
# Ensure required models are present in HF cache for embedding.
# - Whisper (default: LibraxisAI/whisper-large-v3-turbo-mlx-q8)
# - Embedder (default: sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2)

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Prefer explicit path override for Whisper
if [[ -n "${CODESCRIBE_MODEL_PATH:-}" ]]; then
  if [[ -f "${CODESCRIBE_MODEL_PATH}/config.json" ]]; then
    echo "✓ Whisper model found via CODESCRIBE_MODEL_PATH (${CODESCRIBE_MODEL_PATH})"
    WHISPER_OK=1
  fi
fi

# If embed model points to a local directory, treat as satisfied.
if [[ -z "${WHISPER_OK:-}" && -n "${CODESCRIBE_EMBED_MODEL:-}" ]]; then
  if [[ -d "${CODESCRIBE_EMBED_MODEL}" && -f "${CODESCRIBE_EMBED_MODEL}/config.json" ]]; then
    echo "✓ Whisper model found via CODESCRIBE_EMBED_MODEL (${CODESCRIBE_EMBED_MODEL})"
    WHISPER_OK=1
  fi
fi

# Cache search paths (mirrors core/build.rs)
CACHE_DIRS=()
[[ -n "${CODESCRIBE_HF_CACHE:-}" ]] && CACHE_DIRS+=("$CODESCRIBE_HF_CACHE")
[[ -n "${HUGGINGFACE_HUB_CACHE:-}" ]] && CACHE_DIRS+=("$HUGGINGFACE_HUB_CACHE")
[[ -n "${HF_HUB_CACHE:-}" ]] && CACHE_DIRS+=("$HF_HUB_CACHE")
[[ -n "${HF_HOME:-}" ]] && CACHE_DIRS+=("$HF_HOME/hub")
CACHE_DIRS+=("$HOME/.cache/huggingface/hub")
CACHE_DIRS+=("$HOME/.codescribe/embeddings" "$HOME/.codescribe/embeddings/hub")

repo_dir() {
  local repo="$1"
  echo "models--${repo//\//--}"
}

has_snapshot_with_files() {
  local repo="$1"; shift
  local required=("$@")
  for base in "${CACHE_DIRS[@]}"; do
    local dir="$base/$(repo_dir "$repo")/snapshots"
    [[ -d "$dir" ]] || continue
    for snap in "$dir"/*; do
      [[ -d "$snap" ]] || continue
      local ok=1
      for f in "${required[@]}"; do
        if [[ "$f" == "__ANY_SAFETENSORS__" ]]; then
          compgen -G "$snap"/*.safetensors >/dev/null || ok=0
        else
          [[ -f "$snap/$f" ]] || ok=0
        fi
      done
      if [[ "$ok" -eq 1 ]]; then
        return 0
      fi
    done
  done
  return 1
}

ensure_repo() {
  local name="$1"; shift
  local repo="$1"; shift
  local required=("$@")

  if has_snapshot_with_files "$repo" "${required[@]}"; then
    echo "✓ ${name} cached (${repo})"
    return 0
  fi

  echo "▶ ${name} not found in cache; downloading (${repo})..."
  if [[ "$name" == "Whisper" ]]; then
    "$ROOT_DIR/scripts/download-model.sh"
  else
    "$ROOT_DIR/scripts/download-embedder.sh"
  fi
}

WHISPER_REPO="LibraxisAI/whisper-large-v3-turbo-mlx-q8"
if [[ -n "${CODESCRIBE_EMBED_MODEL:-}" && "${CODESCRIBE_EMBED_MODEL}" == */* ]]; then
  WHISPER_REPO="$CODESCRIBE_EMBED_MODEL"
fi
EMBEDDER_REPO="${CODESCRIBE_EMBEDDER_REPO:-sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2}"

# If CODESCRIBE_MODEL_PATH already satisfied, skip Whisper cache check
if [[ "${WHISPER_OK:-0}" -ne 1 ]]; then
  ensure_repo "Whisper" "$WHISPER_REPO" config.json tokenizer.json mel_filters.npz __ANY_SAFETENSORS__
fi

ensure_repo "Embedder" "$EMBEDDER_REPO" config.json tokenizer.json model.safetensors
