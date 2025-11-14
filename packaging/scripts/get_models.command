#!/bin/zsh
# get_models.command
#
# Purpose: Download local models into ./models using the helper script. Useful after first install.
# Usage: double-click to download Whisper large-v3-turbo by default, or run with args.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"

: "${WHISPER_VARIANT:=large-v3-turbo}"

cd "$REPO_DIR"

RUNNER="uv run python"
if ! command -v uv >/dev/null 2>&1; then
  if command -v python3 >/dev/null 2>&1; then
    RUNNER="python3"
  else
    echo "[!] Neither 'uv' nor 'python3' found in PATH." >&2
    exit 1
  fi
fi

if [[ $# -gt 0 ]]; then
  echo "[i] Running: $RUNNER scripts/get_models.py $@"
  eval "$RUNNER scripts/get_models.py \"$@\""
else
  echo "[i] Running: $RUNNER scripts/get_models.py --whisper ${WHISPER_VARIANT}"
  eval "$RUNNER scripts/get_models.py --whisper \"${WHISPER_VARIANT}\""
fi

# Print next-steps hints are already in the helper output
