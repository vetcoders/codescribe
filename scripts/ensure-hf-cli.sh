#!/bin/bash
# Ensure the HuggingFace CLI is available without relying on system-wide pip.

set -euo pipefail

if command -v hf >/dev/null 2>&1; then
  command -v hf
  exit 0
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required to bootstrap the HuggingFace CLI" >&2
  exit 1
fi

CACHE_ROOT="${XDG_CACHE_HOME:-$HOME/.cache}"
VENV_DIR="${CODESCRIBE_HFCLI_VENV:-$CACHE_ROOT/codescribe/hf-cli-venv}"
HF_BIN="$VENV_DIR/bin/hf"

if [[ ! -x "$HF_BIN" ]]; then
  echo "▶ Bootstrapping hf CLI in $VENV_DIR" >&2
  python3 -m venv "$VENV_DIR"
  "$VENV_DIR/bin/python" -m pip install -q --upgrade pip
  "$VENV_DIR/bin/python" -m pip install -q "huggingface_hub[cli]"
fi

echo "$HF_BIN"
