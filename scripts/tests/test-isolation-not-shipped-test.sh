#!/usr/bin/env bash
# Run after workspace tests in the same Cargo target directory. Cargo resolver 2
# may unify dev features while building tests; a separate normal build must not.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"
target_root="${CARGO_TARGET_DIR:-$repo_root/target}"
if [[ "$target_root" != /* ]]; then
    target_root="$repo_root/$target_root"
fi

# Debug profile has the same feature resolution as release. It keeps this
# artifact inspection affordable while using the package selectors from both
# scripts/build-app.sh and Makefile release-codescribe.
env -u CODESCRIBE_LOCAL_INSTALL -u CODESCRIBE_EMBED_WHISPER \
    -u CODESCRIBE_EMBED_EMBEDDER -u CODESCRIBE_NO_EMBED cargo build -p codescribe-ffi
env -u CODESCRIBE_LOCAL_INSTALL cargo build -p codescribe-core --bin codescribe-stt-sidecar

for artifact in "$target_root/debug/libcodescribe_ffi.dylib" \
                "$target_root/debug/codescribe-stt-sidecar"; do
    if [[ ! -f "$artifact" ]]; then
        echo "test isolation ship proof: missing artifact $artifact" >&2
        exit 1
    fi
    if strings -a "$artifact" | /usr/bin/grep -F 'test process refused write under real home' > /dev/null; then
        echo "test isolation ship proof: test fence reached $artifact" >&2
        exit 1
    fi
done
echo "test isolation ship proof: separate normal builds contain no refusal text"
