#!/usr/bin/env bash
set -euo pipefail

repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
stage="${1:-}"
scope="${2:-}"

case "$stage" in
  demolished)
    test -z "$scope" || { echo 'demolished does not accept a scope' >&2; exit 2; }
    exec python3 "$repo/scripts/verify-acoustic-throne-structure.py" demolished \
      --repo "$repo" \
      --canary-manifest "$repo/tests/fixtures/acoustic_authority_canary.json"
    ;;
  assembled)
    test -n "$scope" || { echo 'assembled requires a scope' >&2; exit 2; }
    exec python3 "$repo/scripts/verify-acoustic-throne-structure.py" assembled "$scope" \
      --repo "$repo"
    ;;
  wired)
    test -z "$scope" || { echo 'wired does not accept a scope' >&2; exit 2; }
    exec python3 "$repo/scripts/verify-acoustic-throne-structure.py" wired \
      --repo "$repo"
    ;;
  *)
    echo 'usage: scripts/verify-authority-shape.sh demolished|assembled SCOPE|wired' >&2
    exit 2
    ;;
esac
