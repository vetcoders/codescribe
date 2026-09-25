#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/lib/fixture-audio.sh
. "$script_dir/../lib/fixture-audio.sh"
fixture_root="$(mktemp -d)"
trap 'rm -r "$fixture_root"' EXIT
mkdir -p "$fixture_root/fixtures"
dd if=/dev/zero of="$fixture_root/source.wav" bs=1024 count=1024 2>/dev/null
before="$(stat -f %l "$fixture_root/source.wav")"
stage_fixture_audio "$fixture_root/source.wav" "$fixture_root/fixtures/staged.wav"
after="$(stat -f %l "$fixture_root/source.wav")"
[[ "$after" -eq "$((before + 1))" ]]
[[ "$(stat -f %i "$fixture_root/source.wav")" == "$(stat -f %i "$fixture_root/fixtures/staged.wav")" ]]
printf 'ok: bench fixture WAV shares its inode and adds no second payload\n'
