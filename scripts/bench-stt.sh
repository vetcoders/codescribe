#!/usr/bin/env bash
set -Eeuo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/bench-stt.sh [--fixtures auto|historical|repo] [--limit N] [--out DIR] [--language LANG]

Environment:
  BENCH_STT_FIXTURES   auto|historical|repo (default: auto)
  BENCH_STT_LIMIT      fixture limit for historical corpus (default: 10)
  BENCH_STT_OUT        output directory under ~/.codescribe (default: timestamped report dir)
  BENCH_STT_LANGUAGE   Whisper language code (default: pl)
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

fixture_mode="${BENCH_STT_FIXTURES:-auto}"
fixture_limit="${BENCH_STT_LIMIT:-10}"
language="${BENCH_STT_LANGUAGE:-pl}"
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
  auto|historical|repo) ;;
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
latency_probe_dir="$out_dir/latency-probe"
latency_tsv="$out_dir/streaming-latency.tsv"
latency_log="$out_dir/streaming-latency.log"

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

model_is_complete() {
  local dir="$1"
  [[ -d "$dir" ]] || return 1
  [[ -f "$dir/config.json" ]] || return 1
  [[ -f "$dir/tokenizer.json" ]] || return 1
  [[ -f "$dir/mel_filters.npz" ]] || return 1
  [[ -f "$dir/weights.safetensors" || -f "$dir/model.safetensors" ]] || return 1
}

discover_model() {
  local candidate
  if [[ -n "${CODESCRIBE_MODEL_PATH:-}" ]] && model_is_complete "$CODESCRIBE_MODEL_PATH"; then
    printf '%s\n' "$CODESCRIBE_MODEL_PATH"
    return 0
  fi

  for candidate in \
    "$home_dir/.codescribe/models/whisper-large-v3-turbo-mlx-q8" \
    "$home_dir/.codescribe/models/whisper-large-v3-mlx-q8"; do
    if model_is_complete "$candidate"; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  local hf_base repo snapshot
  for hf_base in \
    "${CODESCRIBE_HF_CACHE:-}" \
    "${HUGGINGFACE_HUB_CACHE:-}" \
    "${HF_HUB_CACHE:-}" \
    "${HF_HOME:+$HF_HOME/hub}" \
    "$home_dir/.cache/huggingface/hub"; do
    [[ -n "$hf_base" ]] || continue
    for repo in \
      models--LibraxisAI--whisper-large-v3-turbo-mlx-q8 \
      models--libraxisai--whisper-large-v3-turbo-mlx-q8 \
      models--LibraxisAI--whisper-large-v3-mlx-q8 \
      models--libraxisai--whisper-large-v3-mlx-q8; do
      for snapshot in "$hf_base/$repo/snapshots"/*; do
        [[ -d "$snapshot" ]] || continue
        if model_is_complete "$snapshot"; then
          printf '%s\n' "$snapshot"
          return 0
        fi
      done
    done
  done

  return 1
}

write_honest_report() {
  local reason="$1"
  local head_short
  head_short="$(git -C "$repo_root" rev-parse --short=8 HEAD 2>/dev/null || printf 'unknown')"
  {
    printf '# CodeScribe STT Baseline Bench\n\n'
    printf '[!] %s\n\n' "$reason"
    printf '## Repro command\n\n'
    printf '```bash\n'
    printf 'scripts/bench-stt.sh --fixtures %s --limit %s --language %s\n' "$fixture_mode" "$fixture_limit" "$language"
    printf '```\n\n'
    printf '## Run context\n\n'
    printf '%s\n' "- repo: \`$repo_root\`"
    printf '%s\n' "- head: \`$head_short\`"
    printf '%s\n' "- fixture mode: \`$fixture_mode\`"
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
      export "$key=$value"
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

  if [[ "$fixture_mode" != "repo" ]]; then
    select_from_benchmark_report
  fi

  if [[ "$(count_lines "$selected_tsv")" -gt 0 ]]; then
    return 0
  fi

  if [[ "$fixture_mode" == "historical" || "$fixture_mode" == "auto" ]]; then
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
  fi
}

select_repo_pairs() {
  : > "$selected_tsv"
  local assets="$repo_root/tests/assets/data_assets"
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

write_latency_probe() {
  mkdir -p "$latency_probe_dir/src"
  cat > "$latency_probe_dir/Cargo.toml" <<EOF
[package]
name = "codescribe-bench-stt-latency"
version = "0.1.0"
edition = "2024"

[dependencies]
codescribe-core = { path = "$repo_root/core" }
anyhow = "1"
EOF

  cat > "$latency_probe_dir/src/main.rs" <<'EOF'
use anyhow::Result;
use codescribe_core::{audio, whisper::LocalWhisperEngine};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn entry_id(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("audio");
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("bench");
    format!("{parent}__{stem}")
}

fn main() -> Result<()> {
    let model_path = PathBuf::from(env::var("CODESCRIBE_MODEL_PATH")?);
    let audio_paths: Vec<PathBuf> = env::args().skip(1).map(PathBuf::from).collect();
    let mut engine = LocalWhisperEngine::new(&model_path)?;

    println!("id\taudio_path\tduration_sec\tfirst_preview_ms\tfinal_ms\tcallbacks\tfinal_chars");

    for audio_path in audio_paths {
        let (samples, sample_rate) = audio::load_audio_file(&audio_path)?;
        let duration_sec = samples.len() as f64 / sample_rate as f64;
        let first_preview_ms = Arc::new(Mutex::new(None::<u128>));
        let callbacks = Arc::new(AtomicUsize::new(0));
        let first_preview_for_callback = Arc::clone(&first_preview_ms);
        let callbacks_for_callback = Arc::clone(&callbacks);
        let started = Instant::now();

        let callback = |text: &str| {
            callbacks_for_callback.fetch_add(1, Ordering::SeqCst);
            if !text.trim().is_empty() {
                let mut guard = first_preview_for_callback
                    .lock()
                    .expect("first preview mutex poisoned");
                if guard.is_none() {
                    *guard = Some(started.elapsed().as_millis());
                }
            }
        };

        let final_text =
            engine.transcribe_long_streaming(&samples, sample_rate, Some("pl"), Some(&callback))?;
        let final_ms = started.elapsed().as_millis();
        let first_ms = first_preview_ms
            .lock()
            .expect("first preview mutex poisoned")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "NA".to_string());

        println!(
            "{}\t{}\t{:.3}\t{}\t{}\t{}\t{}",
            entry_id(&audio_path),
            audio_path.display(),
            duration_sec,
            first_ms,
            final_ms,
            callbacks.load(Ordering::SeqCst),
            final_text.chars().count()
        );
    }

    Ok(())
}
EOF
}

manifest_audio_args() {
  tail -n +2 "$manifest_tsv" | awk -F '\t' '{print $6}'
}

run_qube_report() {
  rm -rf "$qube_out"
  log "running qube-report for WER"
  (
    cd "$repo_root"
    CODESCRIBE_NO_EMBED=1 CODESCRIBE_MODEL_PATH="$model_path" \
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

run_latency_probe() {
  write_latency_probe
  : > "$latency_tsv"
  log "running streaming callback latency probe"
  local audio_args=()
  local audio_path
  while IFS= read -r audio_path; do
    [[ -n "$audio_path" ]] && audio_args+=("$audio_path")
  done < <(manifest_audio_args)

  (
    CODESCRIBE_NO_EMBED=1 CODESCRIBE_MODEL_PATH="$model_path" CARGO_TARGET_DIR="$repo_root/target" \
      cargo run --quiet --manifest-path "$latency_probe_dir/Cargo.toml" -- "${audio_args[@]}"
  ) > "$latency_tsv" 2> "$latency_log"
}

write_summary_report() {
  local head_short repro
  head_short="$(git -C "$repo_root" rev-parse --short=8 HEAD 2>/dev/null || printf 'unknown')"
  repro="scripts/bench-stt.sh --fixtures $fixture_mode --limit $fixture_limit --language $language"
  python3 - "$report_path" "$qube_out/report.json" "$manifest_tsv" "$latency_tsv" "$repro" "$repo_root" "$head_short" "$model_path" "$out_dir" <<'PY'
import csv
import json
import math
import sys
from pathlib import Path

report_path = Path(sys.argv[1])
qube_json = Path(sys.argv[2])
manifest_tsv = Path(sys.argv[3])
latency_tsv = Path(sys.argv[4])
repro = sys.argv[5]
repo_root = sys.argv[6]
head_short = sys.argv[7]
model_path = sys.argv[8]
out_dir = sys.argv[9]

qube = json.loads(qube_json.read_text())
manifest = list(csv.DictReader(manifest_tsv.open(), delimiter="\t"))
latencies = {
    row["id"]: row
    for row in csv.DictReader(latency_tsv.open(), delimiter="\t")
}

def pct(value):
    if value is None:
        return "n/a"
    return f"{value * 100:.2f}%"

def ms(value):
    if value in (None, "", "NA"):
        return "n/a"
    return f"{float(value):.0f}"

def avg(values):
    values = [float(v) for v in values if v not in (None, "", "NA")]
    return sum(values) / len(values) if values else None

def p95(values):
    values = sorted(float(v) for v in values if v not in (None, "", "NA"))
    if not values:
        return None
    idx = max(0, math.ceil(0.95 * len(values)) - 1)
    return values[idx]

summary = qube.get("summary", {})
entries = qube.get("entries", [])
first_values = [row.get("first_preview_ms") for row in latencies.values()]
final_values = [row.get("final_ms") for row in latencies.values()]

lines = []
lines.append("# CodeScribe STT Baseline Bench")
lines.append("")
lines.append("## Co zmierzono")
lines.append("")
lines.append(f"- repo: `{repo_root}`")
lines.append(f"- head: `{head_short}`")
lines.append(f"- model: `{model_path}`")
lines.append(f"- output: `{out_dir}`")
lines.append("- WER: existing `qube-report` over staged WAV/TXT corpus references")
lines.append("- streaming latency: `LocalWhisperEngine::transcribe_long_streaming` callback probe; model load is excluded")
lines.append("")
lines.append("## Komenda repro")
lines.append("")
lines.append("```bash")
lines.append(repro)
lines.append("```")
lines.append("")
lines.append("## Liczby")
lines.append("")
lines.append(f"- files processed: `{summary.get('processed_files', len(entries))}` / `{summary.get('total_files', len(entries))}`")
lines.append(f"- avg raw WER: `{pct(summary.get('avg_raw_wer'))}`")
lines.append(f"- avg post WER: `{pct(summary.get('avg_post_wer'))}`")
lines.append(f"- time-to-first-preview avg/p95: `{ms(avg(first_values))} ms` / `{ms(p95(first_values))} ms`")
lines.append(f"- time-to-final avg/p95: `{ms(avg(final_values))} ms` / `{ms(p95(final_values))} ms`")
lines.append("")
lines.append("| file | raw WER | post WER | first preview ms | final ms | callbacks |")
lines.append("| --- | ---: | ---: | ---: | ---: | ---: |")
for entry in entries:
    row = latencies.get(entry.get("id"), {})
    metrics = entry.get("metrics", {})
    lines.append(
        "| {id} | {raw} | {post} | {first} | {final} | {callbacks} |".format(
            id=entry.get("id", "unknown"),
            raw=pct(metrics.get("raw_wer")),
            post=pct(metrics.get("post_wer")),
            first=ms(row.get("first_preview_ms")),
            final=ms(row.get("final_ms")),
            callbacks=row.get("callbacks", "n/a"),
        )
    )
lines.append("")
lines.append("## Fixtures")
lines.append("")
lines.append("| id | source | sha256(audio) | sha256(reference) |")
lines.append("| --- | --- | --- | --- |")
for row in manifest:
    lines.append(
        f"| {row['id']} | `{row['source_audio']}` / `{row['source_reference']}` | `{row['sha256_audio']}` | `{row['sha256_reference']}` |"
    )
lines.append("")
lines.append("## Czego NIE zweryfikowano")
lines.append("")
lines.append("- Nie uruchomiono pełnego `make test`; zmiana dotyczy tylko skryptu bench.")
lines.append("- Latencja mierzy callback Whisper streaming po załadowaniu modelu, nie pełny czas cold-start aplikacji.")
lines.append("")
lines.append("## Artefakty")
lines.append("")
lines.append(f"- qube report JSON: `{qube_json}`")
lines.append(f"- latency TSV: `{latency_tsv}`")
lines.append(f"- fixture manifest: `{manifest_tsv}`")
lines.append("")
report_path.write_text("\n".join(lines) + "\n")

print("\n".join(lines[:28]))
print(f"\n[bench-stt] report: {report_path}")
PY
}

load_env_file

if ! command -v cargo >/dev/null 2>&1; then
  write_honest_report "cargo is not available; cannot run STT benchmark."
fi
if ! command -v python3 >/dev/null 2>&1; then
  write_honest_report "python3 is not available; cannot prepare fixture/report metadata."
fi

if [[ "$fixture_mode" == "repo" ]]; then
  select_repo_pairs
else
  select_historical_pairs
  if [[ "$(count_lines "$selected_tsv")" -eq 0 && "$fixture_mode" == "auto" ]]; then
    select_repo_pairs
  fi
fi

if [[ "$(count_lines "$selected_tsv")" -eq 0 ]]; then
  write_honest_report "No WAV/TXT fixture pairs found for mode '$fixture_mode'."
fi

stage_fixtures

if [[ "$(($(count_lines "$manifest_tsv") - 1))" -le 0 ]]; then
  write_honest_report "Fixture staging produced no usable WAV/TXT pairs."
fi

if ! model_path="$(discover_model)"; then
  write_honest_report "No complete Whisper model found. Checked CODESCRIBE_MODEL_PATH, ~/.codescribe/models/{whisper-large-v3-turbo-mlx-q8,whisper-large-v3-mlx-q8}, and Hugging Face cache snapshots."
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

if ! run_latency_probe; then
  write_honest_report "streaming latency probe failed; see $latency_log."
fi

if [[ ! -s "$latency_tsv" ]]; then
  write_honest_report "streaming latency probe produced no rows."
fi

write_summary_report
