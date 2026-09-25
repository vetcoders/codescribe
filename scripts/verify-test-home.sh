#!/usr/bin/env bash
# Run hermetic Rust tests with an isolated HOME and fail if Codescribe writes there.
set -euo pipefail

real_home="${HOME:?HOME is required}"
sandbox_root="${CODESCRIBE_TEST_DATA_DIR:?TEST_DATA_DIR_SETUP is required}"
export CARGO_HOME="${CARGO_HOME:-$real_home/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$real_home/.rustup}"
export HOME="$sandbox_root/home"
export CODESCRIBE_TEST_ISOLATION=1
mkdir -p "$HOME"

gate_rc=0
if [[ "${1:-}" == "--fixture-runner" ]]; then
    shift
    "$@" || gate_rc=$?
else
    CODESCRIBE_NO_EMBED=1 CODESCRIBE_DISABLE_KEYCHAIN=1 \
        cargo test --workspace --all-targets || gate_rc=$?
    if [[ $gate_rc -eq 0 ]]; then
        CODESCRIBE_NO_EMBED=1 CODESCRIBE_DISABLE_KEYCHAIN=1 \
            cargo test --workspace --doc || gate_rc=$?
    fi
fi

for codescribe_root in "$HOME/.codescribe" "$HOME/Library/Application Support/Codescribe"; do
    if [[ -e "$codescribe_root" ]]; then
        echo "verify: test process wrote under sandbox HOME: $codescribe_root" >&2
        find "$codescribe_root" -print >&2
        gate_rc=1
    fi
done
exit "$gate_rc"
