#!/usr/bin/env bash
# Replay archived takes into a caller-named candidate dir and compare
# TakeTruth sidecars against the historical day archive.
#
# Usage: truth-regress.sh [-n] <transcriptions-day-dir> [N] [candidate-dir]
#
# Dry-run (-n) lists matching pairs and writes nothing. A full pass links
# audio into the candidate dir under the historical stem, runs
# `codescribe transcribe --raw --no-bus` per file, then
# `qube-report --baseline-dir … --candidate-dir …`.
# Never writes under the source day directory or under transcriptions/.

set -euo pipefail

usage() {
  printf 'usage: %s [-n] <transcriptions-day-dir> [N] [candidate-dir]\n' "$(basename "$0")" >&2
}

dry_run=0
while getopts ':n' opt; do
  case "$opt" in
    n) dry_run=1 ;;
    *)
      usage
      exit 2
      ;;
  esac
done
shift $((OPTIND - 1))

if [[ $# -lt 1 ]]; then
  usage
  exit 2
fi

day_dir=$1
limit=${2:-}
candidate_dir=${3:-}

if [[ ! -d "$day_dir" ]]; then
  printf 'error: day dir is not a directory: %s\n' "$day_dir" >&2
  exit 2
fi

day_abs=$(cd -- "$day_dir" && pwd)

if [[ -n "$limit" ]]; then
  case "$limit" in
    '' | *[!0-9]*)
      printf 'error: N must be a non-negative integer, got %s\n' "$limit" >&2
      exit 2
      ;;
  esac
fi

list_pairs() {
  local day=$1
  local audio sidecar base
  local -a audios
  shopt -s nullglob
  audios=("$day"/*_raw.m4a "$day"/*_raw.wav)
  shopt -u nullglob
  for audio in "${audios[@]}"; do
    [[ -e "$audio" ]] || continue
    base=$(basename -- "$audio")
    case "$base" in
      *_raw.m4a) base=${base%.m4a} ;;
      *_raw.wav) base=${base%.wav} ;;
      *) continue ;;
    esac
    sidecar="$day/${base}.txt.truth.json"
    if [[ -f "$sidecar" ]]; then
      printf '%s\t%s\n' "$audio" "$sidecar"
    fi
  done
}

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
list_pairs "$day_abs" | LC_ALL=C sort >"$tmp"
if [[ -n "$limit" && "$limit" -gt 0 ]]; then
  head -n "$limit" "$tmp" >"${tmp}.n"
  mv "${tmp}.n" "$tmp"
fi

pair_count=$(wc -l <"$tmp" | tr -d ' ')

if [[ "$dry_run" -eq 1 ]]; then
  if [[ "$pair_count" -gt 0 ]]; then
    while IFS="$(printf '\t')" read -r audio sidecar; do
      printf 'PAIR\t%s\t%s\n' "$audio" "$sidecar"
    done <"$tmp"
  fi
  printf 'dry-run: %s pair(s); wrote nothing\n' "$pair_count"
  exit 0
fi

if [[ -z "$candidate_dir" ]]; then
  stamp=$(date +%Y%m%d-%H%M%S)
  candidate_dir="$HOME/.codescribe/reports/truth-regress-${stamp}"
fi

mkdir -p -- "$candidate_dir"
cand_abs=$(cd -- "$candidate_dir" && pwd)

case "$cand_abs" in
  "$day_abs" | "$day_abs"/*)
    printf 'error: candidate-dir must not be inside the day archive\n' >&2
    exit 2
    ;;
  */transcriptions | */transcriptions/*)
    printf 'error: candidate-dir must not be under transcriptions/\n' >&2
    exit 2
    ;;
esac

codescribe_bin=${CODESCRIBE:-codescribe}
qube_bin=${QUBE_REPORT:-qube-report}

if ! command -v "$codescribe_bin" >/dev/null 2>&1; then
  printf 'error: codescribe not on PATH (override with CODESCRIBE=)\n' >&2
  exit 127
fi
if ! command -v "$qube_bin" >/dev/null 2>&1; then
  printf 'error: qube-report not on PATH (override with QUBE_REPORT=)\n' >&2
  exit 127
fi

if [[ "$pair_count" -gt 0 ]]; then
  while IFS="$(printf '\t')" read -r audio _sidecar; do
    name=$(basename -- "$audio")
    link="$cand_abs/$name"
    ln -sfn -- "$audio" "$link"
    "$codescribe_bin" transcribe --raw --no-bus -- "$link"
  done <"$tmp"
fi

"$qube_bin" --baseline-dir "$day_abs" --candidate-dir "$cand_abs" --out "$cand_abs"
