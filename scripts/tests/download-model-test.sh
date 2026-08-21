#!/bin/bash
# Hermetic regression for default Whisper bundle promotion.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/codescribe-download-model-test.XXXXXX")

cleanup() {
  find "$TEST_ROOT" -type f -delete 2>/dev/null || true
  find "$TEST_ROOT" -type l -delete 2>/dev/null || true
  find "$TEST_ROOT" -depth -type d -exec rmdir {} \; 2>/dev/null || true
}
trap cleanup EXIT

make_tiny_weights() {
  local destination="$1"
  local header='{"model.weight":{"dtype":"F16","shape":[1],"data_offsets":[0,2]}}'
  # The header is 65 bytes; safetensors prefixes it with little-endian u64.
  printf '\x41\0\0\0\0\0\0\0%s\0\0' "$header" > "$destination"
}

hf() {
  if [[ "${1:-}" == "auth" ]]; then
    return 1
  fi
  if [[ "${2:-}" == "openai/whisper-large-v3-turbo" ]]; then
    printf '%s\n' "$FAKE_TOKENIZER"
  else
    printf '%s\n' "$FAKE_MODEL_SNAPSHOT"
  fi
}
export -f hf

curl() {
  local destination=""
  while [[ "$#" -gt 0 ]]; do
    if [[ "$1" == "-o" ]]; then
      destination="$2"
      shift 2
    else
      shift
    fi
  done
  cp "$FAKE_MEL_FILTERS" "$destination"
}
export -f curl

export CI=true
ORIGINAL_HOME="$HOME"
export CARGO_HOME="${CARGO_HOME:-$ORIGINAL_HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$ORIGINAL_HOME/.rustup}"
export HOME="$TEST_ROOT/home"
export CODESCRIBE_MODELS_DIR="$TEST_ROOT/models"
export FAKE_TOKENIZER="$ROOT_DIR/core/models/whisper-large-v3-turbo-mlx-q8/tokenizer.json"
export FAKE_MEL_FILTERS="$TEST_ROOT/mel_filters.npz"
mkdir -p "$HOME" "$CODESCRIBE_MODELS_DIR"
xxd -r -p "$ROOT_DIR/tests/fixtures/whisper_mel_filters.npz.hex" > "$FAKE_MEL_FILTERS"

run_promotion_case() {
  local selected_name="$1"
  local stale_name="$2"
  local snapshot="$TEST_ROOT/snapshot-$selected_name"
  local destination="$CODESCRIBE_MODELS_DIR/whisper-large-v3-turbo"

  mkdir -p "$snapshot" "$destination"
  printf '{}\n' > "$snapshot/config.json"
  make_tiny_weights "$snapshot/$selected_name"
  make_tiny_weights "$destination/$stale_name"
  export FAKE_MODEL_SNAPSHOT="$snapshot"

  "$ROOT_DIR/scripts/download-model.sh" >/dev/null

  [[ -f "$destination/$selected_name" ]]
  [[ ! -e "$destination/$stale_name" ]]
}

run_promotion_case model.safetensors weights.safetensors
run_promotion_case weights.safetensors model.safetensors

echo "download-model alternate-weight promotion: PASS"
