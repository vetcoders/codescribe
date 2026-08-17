#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
GATE="$ROOT/scripts/developer-surface-gate.sh"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

empty="$WORKDIR/empty"
mkdir -p "$empty"
got="$(
  CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE="$empty/sparkle-public.b64" \
  CODESCRIBE_LICENSE_PUBLIC_KEY_FILE="$empty/license-public.hex" \
  env -u SPARKLE_ED_PUBLIC_KEY -u CODESCRIBE_LICENSE_PUBLIC_KEY_HEX \
    "$GATE"
)"
[[ "$got" == "0" ]] || { echo "expected 0 on empty secrets, got $got" >&2; exit 1; }

good="$WORKDIR/good"
mkdir -p "$good"
printf '%s' 'abcdefghijklmnopqrstuvwxyz0123456789+/AB==' >"$good/sparkle-public.b64"
printf '%s' '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' >"$good/license-public.hex"
got="$(
  CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE="$good/sparkle-public.b64" \
  CODESCRIBE_LICENSE_PUBLIC_KEY_FILE="$good/license-public.hex" \
  env -u SPARKLE_ED_PUBLIC_KEY -u CODESCRIBE_LICENSE_PUBLIC_KEY_HEX \
    "$GATE"
)"
[[ "$got" == "1" ]] || { echo "expected 1 on both keys, got $got" >&2; exit 1; }

got="$(
  CODESCRIBE_DEVELOPER_SURFACE=0 \
  CODESCRIBE_SPARKLE_PUBLIC_KEY_FILE="$good/sparkle-public.b64" \
  CODESCRIBE_LICENSE_PUBLIC_KEY_FILE="$good/license-public.hex" \
    "$GATE"
)"
[[ "$got" == "0" ]] || { echo "expected 0 when forced off, got $got" >&2; exit 1; }

echo "developer-surface-gate: ok"
