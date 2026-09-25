#!/usr/bin/env bash

# Only replace files created by this download. Existing user model directories
# are read for comparison and never mutated.
share_new_model_tokenizers() {
  local new_dir="$1" peer_dir="$2" candidate peer linked
  for candidate in "$new_dir"/tokenizer-*.safetensors; do
    [[ -f "$candidate" ]] || continue
    peer="$peer_dir/$(basename "$candidate")"
    [[ -f "$peer" ]] || continue
    [[ "$(stat -f %d "$candidate")" == "$(stat -f %d "$peer")" ]] || continue
    [[ "$(stat -f %i "$candidate")" != "$(stat -f %i "$peer")" ]] || continue
    cmp -s "$candidate" "$peer" || continue
    linked="${candidate}.linked-$$"
    ln "$peer" "$linked"
    mv -f "$linked" "$candidate"
    printf 'Shared byte-identical tokenizer: %s ⇔ %s\n' "$candidate" "$peer"
  done
}
