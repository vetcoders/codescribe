#!/usr/bin/env bash
set -euo pipefail
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$script_dir/../lib/share-new-model-tokenizer.sh"
fixture_root="$(mktemp -d)"
trap 'rm -r "$fixture_root"' EXIT
mkdir -p "$fixture_root/old" "$fixture_root/new"
printf 'same tokenizer bytes' > "$fixture_root/old/tokenizer-test.safetensors"
cp "$fixture_root/old/tokenizer-test.safetensors" "$fixture_root/new/tokenizer-test.safetensors"
old_inode="$(stat -f %i "$fixture_root/old/tokenizer-test.safetensors")"
share_new_model_tokenizers "$fixture_root/new" "$fixture_root/old"
[[ "$(stat -f %i "$fixture_root/new/tokenizer-test.safetensors")" == "$old_inode" ]]
printf 'ok: new tokenizer shares the existing inode\n'
