#!/usr/bin/env bash
# Hermetic checks for scripts/commit-msg-provenance.sh. No git, no network.
#
# The gate exists to keep authorship on every commit. It earns that only if it
# accepts what git itself writes: blocking a machine-generated `Revert "..."`
# line teaches the author to reach for --no-verify, which defeats the gate
# entirely. So a revert INHERITS the provenance of what it reverts, and a
# revert of an untagged commit is still blocked.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HOOK="$ROOT/scripts/commit-msg-provenance.sh"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
MSG="$WORKDIR/msg"
failures=0

expect() {
  local want="$1" line="$2"
  printf '%s\n' "$line" >"$MSG"
  # Both streams named explicitly: the gate speaks on stderr, and this test
  # judges by exit status alone.
  if sh "$HOOK" "$MSG" >/dev/null 2>/dev/null; then got=accept; else got=block; fi
  if [ "$got" != "$want" ]; then
    echo "want $want, got $got: $line" >&2
    failures=$((failures + 1))
  fi
}

expect accept '[claude/vc-implement] fix(bridge): schema'
expect accept '[codex/vc-ownership] release: embed models by default'
expect accept '[ok-commit] fix: overlay crash'
expect accept "Merge branch 'feature' into develop"
expect accept 'Squashed commit of the following:'

# What git writes on its own, and what blocked a real commit on 2026-08-28.
expect accept 'Revert "[claude/vc-implement] feat(overlay): swift"'
expect accept 'Revert "Revert "[codex/vc-workflow] fix(stt): coarse timing""'
expect accept "Revert \"Merge branch 'feature' into develop\""

# A revert cannot invent provenance the original never had.
expect block 'Revert "an untagged commit"'
expect block 'Revert ""'
# Only the exact wrapper is a revert; prose that merely starts with the word
# must not slip through the peeling loop.
expect block 'Revertowanie czegos'
expect block 'fix: no tag at all'
expect block ''

if [ "$failures" -ne 0 ]; then
  echo "commit-msg-provenance: $failures case(s) wrong" >&2
  exit 1
fi
echo "commit-msg-provenance: ok"
