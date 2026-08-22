#!/bin/bash
# Run the runtime-owned Whisper bundle validator or resolver for shell clients.

set -euo pipefail

if [[ "$#" -ne 1 ]]; then
  echo "usage: $0 <model-directory>|--resolve" >&2
  exit 2
fi

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

cd "$ROOT_DIR"
CODESCRIBE_NO_EMBED=1 cargo run --quiet -p codescribe-core \
  --bin codescribe-whisper-validate -- "$1"
