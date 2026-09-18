#!/usr/bin/env bash
set -euo pipefail

repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

exec env \
  CODESCRIBE_NO_EMBED=1 \
  CODESCRIBE_TEST_DATA_DIR=/tmp/codescribe-p0-b-test-data-141140 \
  cargo test -p codescribe --lib p0_b_five_iwo -- --nocapture
