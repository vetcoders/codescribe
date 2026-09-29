#!/usr/bin/env bash
set -euo pipefail

repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

# Isolation works the way verify-test-home.sh isolates: the app resolves its
# data under HOME, so the witness gets a sandbox HOME of its own.
# CODESCRIBE_TEST_DATA_DIR alone is consumed by no Rust code path.
sandbox="$(mktemp -d /tmp/codescribe-p0-b-five-iwo.XXXXXX)"
trap 'rm -rf "$sandbox"' EXIT
mkdir -p "$sandbox/home"

env \
  CODESCRIBE_NO_EMBED=1 \
  CODESCRIBE_TEST_DATA_DIR="$sandbox" \
  CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" \
  RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" \
  HOME="$sandbox/home" \
  cargo test -p codescribe --lib p0_b_five_iwo -- --nocapture
