#!/usr/bin/env bash
set -Eeuo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/bench-stt.sh [--fixtures repo|historical] [--limit N] [--out DIR] [--language LANG]
                       [--list-fixtures]

Environment:
  BENCH_STT_FIXTURES   repo|historical (default: repo)
  BENCH_STT_LIMIT      fixture limit for historical corpus (default: 10)
  BENCH_STT_OUT        output directory under ~/.codescribe (default: timestamped report dir)
  BENCH_STT_LANGUAGE   Whisper language code (default: pl)
  Whisper model discovery follows the production resolver, including supported
  Hugging Face cache snapshots.
EOF
}

log() {
  printf '[bench-stt] %s\n' "$*"
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if repo_root="$(git -C "$script_dir/.." rev-parse --show-toplevel 2>/dev/null | sed -n '1p')" && [[ -n "$repo_root" ]]; then
  :
else
  repo_root="$(cd -- "$script_dir/.." && pwd)"
fi
home_dir="${HOME:-}"
model_validator="$repo_root/scripts/validate-whisper-model.sh"

fixture_mode="${BENCH_STT_FIXTURES:-repo}"
fixture_limit="${BENCH_STT_LIMIT:-10}"
language="${BENCH_STT_LANGUAGE:-pl}"
list_fixtures=false
run_id="$(date '+%Y%m%d-%H%M%S')-$$"
out_dir="${BENCH_STT_OUT:-$home_dir/.codescribe/reports/bench-stt-baseline-$run_id}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --fixtures)
      fixture_mode="${2:-}"
      shift 2
      ;;
    --limit)
      fixture_limit="${2:-}"
      shift 2
      ;;
    --out)
      out_dir="${2:-}"
      shift 2
      ;;
    --language)
      language="${2:-}"
      shift 2
      ;;
    --list-fixtures)
      list_fixtures=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'Unknown argument: %s\n\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$fixture_mode" in
  historical|repo) ;;
  *)
    printf 'Invalid --fixtures value: %s\n' "$fixture_mode" >&2
    exit 2
    ;;
esac

case "$fixture_limit" in
  ''|*[!0-9]*)
    printf 'Invalid --limit value: %s\n' "$fixture_limit" >&2
    exit 2
    ;;
esac

if [[ -z "$home_dir" ]]; then
  printf 'HOME is not set; cannot locate ~/.codescribe.\n' >&2
  exit 2
fi

case "$out_dir" in
  "$home_dir/.codescribe"/*) ;;
  *)
    printf 'Output dir must stay under ~/.codescribe: %s\n' "$out_dir" >&2
    exit 2
    ;;
esac

mkdir -p "$out_dir"

report_path="$out_dir/bench-report.md"
selected_tsv="$out_dir/selected-fixtures.tsv"
manifest_tsv="$out_dir/fixtures.tsv"
stage_root="$out_dir/fixtures"
qube_out="$out_dir/qube-report"
qube_log="$out_dir/qube-report.log"

count_lines() {
  if [[ -f "$1" ]]; then
    wc -l < "$1" | tr -d '[:space:]'
  else
    printf '0'
  fi
}

sha256_file() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    sha256sum "$1" | awk '{print $1}'
  fi
}

discover_model() {
  "$model_validator" --resolve
}

fixture_source_label() {
  local source_file="${1:-$selected_tsv}"
  local sources
  if [[ -s "$source_file" ]]; then
    sources="$(awk -F '\t' '
      NR == 1 {
        for (i = 1; i <= NF; i++) {
          if ($i == "source") {
            source_col = i
            next
          }
        }
        source_col = 4
      }
      NF >= source_col && $source_col != "" {print $source_col}
    ' "$source_file" | sort -u | paste -sd, -)"
    if [[ -n "$sources" ]]; then
      printf '%s\n' "$sources"
      return 0
    fi
  fi
  printf '%s\n' "$fixture_mode"
}

write_honest_report() {
  local reason="$1"
  local head_short fixture_source
  head_short="$(git -C "$repo_root" rev-parse --short=8 HEAD 2>/dev/null || printf 'unknown')"
  fixture_source="$(fixture_source_label)"
  {
    printf '# Codescribe STT Baseline Bench\n\n'
    printf '[!] %s\n\n' "$reason"
    printf '## Repro command\n\n'
    printf '```bash\n'
    printf 'scripts/bench-stt.sh --fixtures %s --limit %s --language %s\n' "$fixture_mode" "$fixture_limit" "$language"
    printf '```\n\n'
    printf '## Run context\n\n'
    printf '%s\n' "- repo: \`$repo_root\`"
    printf '%s\n' "- head: \`$head_short\`"
    printf '%s\n' "- fixture mode: \`$fixture_mode\`"
    printf '%s\n' "- fixture_source: \`$fixture_source\`"
    printf '%s\n' "- output: \`$out_dir\`"
    printf '\n## Fixture manifest\n\n'
    if [[ -s "$manifest_tsv" ]]; then
      printf '```tsv\n'
      cat "$manifest_tsv"
      printf '```\n'
    else
      printf 'No fixture manifest was produced.\n'
    fi
  } > "$report_path"

  log "[!] $reason"
  log "report: $report_path"
  exit 0
}

load_env_file() {
  local env_file="$home_dir/.codescribe/.env"
  local line key value
  if [[ -r "$env_file" ]]; then
    while IFS= read -r line || [[ -n "$line" ]]; do
      line="${line%$'\r'}"
      [[ "$line" =~ ^[[:space:]]*$ ]] && continue
      [[ "$line" =~ ^[[:space:]]*# ]] && continue
      [[ "$line" =~ ^[[:space:]]*([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]] || continue
      key="${BASH_REMATCH[1]}"
      value="${BASH_REMATCH[2]}"
      value="${value#"${value%%[![:space:]]*}"}"
      value="${value%"${value##*[![:space:]]}"}"
      if [[ "$value" == \"*\" && "$value" == *\" ]]; then
        value="${value:1:${#value}-2}"
      elif [[ "$value" == \'*\' && "$value" == *\' ]]; then
        value="${value:1:${#value}-2}"
      fi
      # Explicit harness pins win over the operator dotenv. A benchmark must
      # not silently switch lanes because ~/.codescribe/.env was edited.
      if [[ -z "${!key+x}" ]]; then
        export "$key=$value"
      fi
    done < "$env_file"
  fi
}

select_from_benchmark_report() {
  local report_json="$home_dir/.codescribe/reports/benchmark_candle_20260211/report.json"
  [[ -f "$report_json" ]] || return 0
  python3 - "$report_json" "$fixture_limit" > "$selected_tsv" <<'PY'
import json
import sys
from pathlib import Path

report = Path(sys.argv[1])
limit = int(sys.argv[2])
data = json.loads(report.read_text())
count = 0
for entry in data.get("entries", []):
    audio = Path(entry.get("audio_path", ""))
    ref = Path(entry.get("reference_path", ""))
    if not audio.exists() or not ref.exists():
        continue
    print(f"{entry.get('id', audio.stem)}\t{audio}\t{ref}\tbenchmark_candle_20260211")
    count += 1
    if limit > 0 and count >= limit:
        break
PY
}

select_historical_pairs() {
  : > "$selected_tsv"

  select_from_benchmark_report

  if [[ "$(count_lines "$selected_tsv")" -gt 0 ]]; then
    return 0
  fi

  local all_tsv="$out_dir/historical-candidates.tsv"
  : > "$all_tsv"
  local dir wav ref id
  for dir in \
    "$home_dir/.codescribe/transcriptions/2026-02-11" \
    "$home_dir/.codescribe/transcriptions/2026-01-17"; do
    [[ -d "$dir" ]] || continue
    for wav in "$dir"/*.wav; do
      [[ -f "$wav" ]] || continue
      ref="${wav%.wav}.txt"
      [[ -f "$ref" ]] || continue
      id="$(basename "$dir")__$(basename "${wav%.wav}")"
      printf '%s\t%s\t%s\thistorical_scan\n' "$id" "$wav" "$ref" >> "$all_tsv"
    done
  done
  if [[ "$fixture_limit" -eq 0 ]]; then
    sort "$all_tsv" > "$selected_tsv"
  else
    sort "$all_tsv" | head -n "$fixture_limit" > "$selected_tsv"
  fi
}

select_repo_pairs() {
  : > "$selected_tsv"
  # Private fixtures are local-only (see tests/assets/data_assets/README.md).
  local assets="${CODESCRIBE_DATA_ASSETS:-$HOME/.codescribe/data_assets}"
  [[ -d "$assets" ]] || assets="$repo_root/tests/assets/data_assets"
  local stem wav ref
  for stem in \
    01_no-to-dobra \
    02_kubernetes-wymaga-konfiguracji \
    03_algorytm-ma-zlozonosc \
    04_runda-3-czyli; do
    wav="$assets/$stem.wav"
    ref="$assets/${stem}_human_transcription.txt"
    if [[ -f "$wav" && -f "$ref" ]]; then
      printf 'repo-assets__%s\t%s\t%s\trepo_tests_assets\n' "$stem" "$wav" "$ref" >> "$selected_tsv"
    fi
  done
}

stage_fixtures() {
  rm -rf "$stage_root"
  mkdir -p "$stage_root"
  {
    printf 'id\tsource_audio\tsha256_audio\tsource_reference\tsha256_reference\tstaged_audio\tstaged_reference\tsource\n'
  } > "$manifest_tsv"

  local id audio ref source date_dir stem staged_dir staged_audio staged_ref
  while IFS=$'\t' read -r id audio ref source; do
    [[ -n "$id" && -f "$audio" && -f "$ref" ]] || continue
    date_dir="${id%%__*}"
    stem="${id#*__}"
    if [[ "$stem" == "$id" ]]; then
      date_dir="bench"
      stem="$id"
    fi
    staged_dir="$stage_root/$date_dir"
    staged_audio="$staged_dir/$stem.wav"
    staged_ref="$staged_dir/$stem.txt"
    mkdir -p "$staged_dir"
    cp -p "$audio" "$staged_audio"
    cp -p "$ref" "$staged_ref"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "${date_dir}__${stem}" \
      "$audio" "$(sha256_file "$audio")" \
      "$ref" "$(sha256_file "$ref")" \
      "$staged_audio" "$staged_ref" "$source" >> "$manifest_tsv"
  done < "$selected_tsv"
}

list_selected_fixtures() {
  local fixture_source
  fixture_source="$(fixture_source_label)"
  printf 'fixture_source\t%s\n' "$fixture_source"
  printf 'id\tsource_audio\tsource_reference\tsource\n'
  cat "$selected_tsv"
}

run_qube_report() {
  rm -rf "$qube_out"
  log "running Qube quality control"
  (
    cd "$repo_root"
    CODESCRIBE_DISABLE_KEYCHAIN=1 CODESCRIBE_NO_EMBED=1 CODESCRIBE_MODEL_PATH="$model_path" \
      cargo run --quiet --bin qube-report -- \
        --input "$stage_root" \
        --out "$qube_out" \
        --limit 0 \
        --language "$language" \
        --skip-cloud \
        --skip-formatting \
        --no-embeddings \
        --metrics-reference corpus
  ) 2>&1 | tee "$qube_log"
}

write_summary_report() {
  local head_short repro fixture_source
  head_short="$(git -C "$repo_root" rev-parse --short=8 HEAD 2>/dev/null || printf 'unknown')"
  repro="scripts/bench-stt.sh --fixtures $fixture_mode --limit $fixture_limit --language $language"
  fixture_source="$(fixture_source_label "$manifest_tsv")"
  python3 - "$report_path" "$qube_out/report.json" "$manifest_tsv" "$repro" "$repo_root" "$head_short" "$model_path" "$out_dir" "$fixture_source" <<'PY'
import csv
import json
import sys
from pathlib import Path

report_path = Path(sys.argv[1])
qube_json = Path(sys.argv[2])
manifest_tsv = Path(sys.argv[3])
repro = sys.argv[4]
repo_root = sys.argv[5]
head_short = sys.argv[6]
model_path = sys.argv[7]
out_dir = sys.argv[8]
fixture_source = sys.argv[9]

qube = json.loads(qube_json.read_text())
manifest = list(csv.DictReader(manifest_tsv.open(), delimiter="\t"))
def pct(value):
    if value is None:
        return "n/a"
    return f"{float(value) * 100:.2f}%"

summary = qube.get("summary", {})
entries = qube.get("entries", [])

lines = [
    "# Codescribe STT Real-Path Bench",
    "",
    "## Run Context",
    "",
    f"- repo: `{repo_root}`",
    f"- head: `{head_short}`",
    f"- model: `{model_path}`",
    f"- output: `{out_dir}`",
    f"- fixtures: `{len(manifest)}`",
    f"- fixture_source: `{fixture_source}`",
    "- quality: Qube raw and delivered transcript metrics",
    "- latency: not measured by this quality bench",
    "- vocabulary-prompt A/B: not measured because no production runtime owner currently emits that prompt",
    "",
    "## Repro Command",
    "",
    "```bash",
    repro,
    "```",
    "",
    "## Summary Metrics",
    "",
    "| metric | value |",
    "| --- | ---: |",
    f"| Qube raw WER | {pct(summary.get('avg_raw_wer'))} |",
    f"| Qube delivered WER | {pct(summary.get('avg_post_wer'))} |",
    "",
    "## Per-Fixture Metrics",
    "",
    "| file | raw WER | delivered WER |",
    "| --- | ---: | ---: |",
]
for entry in entries:
    metrics = entry.get("metrics", {})
    lines.append(
        "| {id} | {raw} | {delivered} |".format(
            id=entry.get("id", "unknown"),
            raw=pct(metrics.get("raw_wer")),
            delivered=pct(metrics.get("post_wer")),
        )
    )

lines.extend([
    "",
    "## Fixtures",
    "",
    "| id | source | sha256(audio) | sha256(reference) |",
    "| --- | --- | --- | --- |",
])
for row in manifest:
    lines.append(
        f"| {row['id']} | `{row['source_audio']}` / `{row['source_reference']}` | "
        f"`{row['sha256_audio']}` | `{row['sha256_reference']}` |"
    )

lines.extend([
    "",
    "## Not Verified Here",
    "",
    "- This report does not compare a vocabulary prompt because the current production lanes emit none.",
    "- This quality report does not measure production streaming latency.",
    "",
    "## Artifacts",
    "",
    f"- Qube report JSON: `{qube_json}`",
    f"- fixture manifest: `{manifest_tsv}`",
    "",
])
report_path.write_text("\n".join(lines))
print("\n".join(lines[:34]))
print(f"\n[bench-stt] report: {report_path}")
PY
}
load_env_file

if [[ "$fixture_mode" == "historical" ]] && ! command -v python3 >/dev/null 2>&1; then
  write_honest_report "python3 is not available; cannot read historical benchmark fixtures."
fi

case "$fixture_mode" in
  repo)
    select_repo_pairs
    ;;
  historical)
    select_historical_pairs
    ;;
esac

if [[ "$(count_lines "$selected_tsv")" -eq 0 ]]; then
  write_honest_report "No WAV/TXT fixture pairs found for mode '$fixture_mode'."
fi

if [[ "$list_fixtures" == "true" ]]; then
  list_selected_fixtures
  exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
  write_honest_report "cargo is not available; cannot run STT benchmark."
fi
if ! command -v python3 >/dev/null 2>&1; then
  write_honest_report "python3 is not available; cannot prepare fixture/report metadata."
fi

stage_fixtures

if [[ "$(($(count_lines "$manifest_tsv") - 1))" -le 0 ]]; then
  write_honest_report "Fixture staging produced no usable WAV/TXT pairs."
fi

if ! model_path="$(discover_model)"; then
  write_honest_report "No complete fp16 Whisper model found by the production resolver, including supported Hugging Face cache snapshots."
fi

export CODESCRIBE_MODEL_PATH="$model_path"

log "repo: $repo_root"
log "head: $(git -C "$repo_root" rev-parse --short=8 HEAD 2>/dev/null || printf 'unknown')"
log "fixtures: $(($(count_lines "$manifest_tsv") - 1))"
log "model: $model_path"
log "output: $out_dir"

if ! run_qube_report; then
  write_honest_report "qube-report failed; see $qube_log."
fi

if [[ ! -f "$qube_out/report.json" ]]; then
  write_honest_report "qube-report finished without report.json."
fi

write_summary_report
