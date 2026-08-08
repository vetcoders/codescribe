#!/usr/bin/env bash
# ============================================================================
# data-assets.sh — the ONE fixture resolution order, for shell consumers
# ============================================================================
# `tests/assets/data_assets/README.md` documents a three-tier order for the
# private STT fixtures (real operator speech, deprivatized twice — never in
# git):
#
#   1. $CODESCRIBE_DATA_ASSETS  — explicit fixtures directory
#   2. ~/.codescribe/data_assets — the canonical local home
#   3. <repo>/tests/assets/data_assets — gitignored in-repo drop dir
#
# The Rust e2e tests implement it (`data_assets_dir()` in
# e2e_overlay_delivery_parity.rs, e2e_full_pipeline.rs, tail_silence_contract.rs)
# and so does scripts/bench-stt.sh. The shell harness did NOT: both
# scripts/e2e-blackhole-dictation.sh and the ENGINE_* Makefile targets knew
# only tier 3 — the tier gitignore keeps empty by design — so
# `make test-engine-parity` died on
#   fixture not found: tests/assets/data_assets/05_apple-live-parity.wav
# on a host whose home corpus holds that exact fixture. This file closes that
# gap in one place instead of four.
#
# Usage (executable):
#   scripts/lib/data-assets.sh dir              → active fixtures directory
#   scripts/lib/data-assets.sh resolve <name>   → absolute path to one fixture
#
# Usage (sourceable):
#   . scripts/lib/data-assets.sh
#   dir="$(data_assets_dir)"
#   clip="$(resolve_data_asset 05_apple-live-parity.wav)"
#
# `resolve` accepts a bare basename, a repo-relative path, or an absolute path.
# An existing path is honored verbatim (an operator who names a real file means
# that file); anything else is matched by basename across the three tiers.
#
# Exit: 0 resolved · 3 not found (every checked path is listed on stderr — a
# cold worker must never have to run a scavenger hunt).
#
# Contract tests: tests/data_assets_resolution.rs

# Logical `cd` (bash default) on purpose: `pwd -P` would rewrite macOS
# /var/folders temp paths to /private/var and break caller string comparisons.
_data_assets_repo_root() {
  local here
  here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)" || return 1
  printf '%s' "$here"
}

# Candidate directories in documented precedence, one per line. Tiers that do
# not exist are still emitted: the not-found message must name every path we
# looked at, not just the ones that happened to be there.
data_assets_candidates() {
  if [ -n "${CODESCRIBE_DATA_ASSETS:-}" ]; then
    printf '%s\n' "$CODESCRIBE_DATA_ASSETS"
  fi
  if [ -n "${HOME:-}" ]; then
    printf '%s\n' "$HOME/.codescribe/data_assets"
  fi
  printf '%s\n' "$(_data_assets_repo_root)/tests/assets/data_assets"
}

# First candidate that is a directory; the in-repo drop dir is the guaranteed
# floor so callers interpolating "$(data_assets_dir)/clip.wav" can never build
# a path starting with a bare slash.
data_assets_dir() {
  local dir last=""
  while IFS= read -r dir; do
    [ -n "$dir" ] || continue
    last="$dir"
    if [ -d "$dir" ]; then
      printf '%s\n' "$dir"
      return 0
    fi
  done < <(data_assets_candidates)
  printf '%s\n' "$last"
}

resolve_data_asset() {
  local wanted="${1:-}"
  if [ -z "$wanted" ]; then
    printf 'resolve_data_asset: missing fixture name\n' >&2
    return 2
  fi

  # An existing path wins outright — including a relative one, which the
  # harness resolves against the repo root it already cd'd into.
  if [ -f "$wanted" ]; then
    printf '%s\n' "$wanted"
    return 0
  fi

  local base="${wanted##*/}"
  local dir checked=()
  while IFS= read -r dir; do
    [ -n "$dir" ] || continue
    checked+=("$dir/$base")
    if [ -f "$dir/$base" ]; then
      printf '%s\n' "$dir/$base"
      return 0
    fi
  done < <(data_assets_candidates)

  {
    printf 'fixture not found: %s\n' "$wanted"
    printf 'checked, in documented order (tests/assets/data_assets/README.md):\n'
    printf '  %s\n' "${checked[@]}"
    printf 'These are private recordings and are never committed. Populate\n'
    printf '~/.codescribe/data_assets or point CODESCRIBE_DATA_ASSETS at the corpus.\n'
  } >&2
  return 3
}

# Executed rather than sourced → dispatch a subcommand.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  set -uo pipefail
  case "${1:-}" in
    dir)
      data_assets_dir
      ;;
    resolve)
      resolve_data_asset "${2:-}"
      ;;
    *)
      printf 'usage: %s {dir|resolve <fixture>}\n' "${0##*/}" >&2
      exit 2
      ;;
  esac
fi
