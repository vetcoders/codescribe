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
  CODESCRIBE_BENCH_FORCE_PROMPT
                       generate a deterministic controlled prompt when runtime prompt is disabled
  CODESCRIBE_BENCH_RUNTIME_PROMPT_TERM_LIMIT
                       trim active runtime prompt to first N deterministic terms (unset = full prompt)
  CODESCRIBE_BENCH_PROMPT_MAX_WER_DELTA_PP
                       fail active-prompt probe above this WER regression threshold (default: 5.0)
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
wer_probe_dir="$out_dir/prompted-wer-probe"
latency_tsv="$out_dir/scheduler-lane-latency.tsv"
latency_log="$out_dir/scheduler-lane-latency.log"
wer_tsv="$out_dir/prompted-wer.tsv"
term_hits_tsv="$out_dir/term-hit-rate.tsv"
term_source_tsv="$out_dir/term-source.tsv"
wer_probe_log="$out_dir/prompted-wer.log"

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

list_selected_fixtures() {
  local fixture_source
  fixture_source="$(fixture_source_label)"
  printf 'fixture_source\t%s\n' "$fixture_source"
  printf 'id\tsource_audio\tsource_reference\tsource\n'
  cat "$selected_tsv"
}

write_wer_probe() {
  mkdir -p "$wer_probe_dir/src"
  cat > "$wer_probe_dir/Cargo.toml" <<EOF
[package]
name = "codescribe-bench-stt-prompted-wer"
version = "0.1.0"
edition = "2024"

[dependencies]
codescribe-core = { path = "$repo_root/core" }
anyhow = "1"
EOF

  cat > "$wer_probe_dir/src/main.rs" <<'EOF'
use anyhow::{Context, Result};
use codescribe_core::{audio, stream_postprocess, whisper};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
struct Fixture {
    id: String,
    audio_path: PathBuf,
    reference_path: PathBuf,
    reference_tokens: Vec<String>,
    reference_norm: String,
}

fn tsv_clean(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\t' | '\r' | '\n' => ' ',
            _ => ch,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn normalize_for_eval(text: &str) -> (Vec<String>, String) {
    let mut normalized = String::with_capacity(text.len());
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() || ch.is_whitespace() {
            normalized.push(ch);
        } else {
            normalized.push(' ');
        }
    }
    let tokens: Vec<String> = normalized
        .split_whitespace()
        .map(|t| t.to_string())
        .collect();
    let normalized = tokens.join(" ");
    (tokens, normalized)
}

fn word_error_rate(reference: &[String], hypothesis: &[String]) -> f32 {
    let dist = levenshtein(reference, hypothesis);
    let denom = reference.len().max(1) as f32;
    dist as f32 / denom
}

fn char_error_rate(reference: &str, hypothesis: &str) -> f32 {
    let ref_chars: Vec<char> = reference.chars().collect();
    let hyp_chars: Vec<char> = hypothesis.chars().collect();
    let dist = levenshtein(&ref_chars, &hyp_chars);
    let denom = ref_chars.len().max(1) as f32;
    dist as f32 / denom
}

fn levenshtein<T: Eq>(a: &[T], b: &[T]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];

    for (i, item_a) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, item_b) in b.iter().enumerate() {
            let cost = if item_a == item_b { 0 } else { 1 };
            cur[j + 1] =
                std::cmp::min(std::cmp::min(prev[j + 1] + 1, cur[j] + 1), prev[j] + cost);
        }
        prev.clone_from(&cur);
    }

    prev[b.len()]
}

fn contains_term(norm_text: &str, term: &str) -> bool {
    let (_, term_norm) = normalize_for_eval(term);
    if term_norm.is_empty() {
        return false;
    }
    let haystack = format!(" {norm_text} ");
    let needle = format!(" {term_norm} ");
    haystack.contains(&needle)
}

fn prompt_terms(prompt: Option<&str>) -> Vec<String> {
    let Some(prompt) = prompt else {
        return Vec::new();
    };
    let body = prompt
        .strip_prefix("Vocabulary:")
        .unwrap_or(prompt)
        .trim()
        .trim_end_matches('.');
    body.split(';')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn env_truthy(key: &str) -> bool {
    env::var(key).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "enabled"
        )
    })
}

fn env_f64(key: &str, default: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .unwrap_or(default)
}

fn env_optional_usize(key: &str) -> Option<usize> {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
}

fn read_manifest(path: &Path) -> Result<Vec<Fixture>> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    let header = lines.next().context("fixture manifest is empty")?;
    let columns: Vec<&str> = header.split('\t').collect();
    let id_idx = columns
        .iter()
        .position(|name| *name == "id")
        .context("fixture manifest lacks id column")?;
    let audio_idx = columns
        .iter()
        .position(|name| *name == "staged_audio")
        .context("fixture manifest lacks staged_audio column")?;
    let reference_idx = columns
        .iter()
        .position(|name| *name == "staged_reference")
        .context("fixture manifest lacks staged_reference column")?;

    let mut fixtures = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let Some(id) = fields.get(id_idx) else {
            continue;
        };
        let Some(audio_path) = fields.get(audio_idx) else {
            continue;
        };
        let Some(reference_path) = fields.get(reference_idx) else {
            continue;
        };
        let reference_text = fs::read_to_string(reference_path)?;
        let (reference_tokens, reference_norm) = normalize_for_eval(&reference_text);
        fixtures.push(Fixture {
            id: (*id).to_string(),
            audio_path: PathBuf::from(audio_path),
            reference_path: PathBuf::from(reference_path),
            reference_tokens,
            reference_norm,
        });
    }
    Ok(fixtures)
}

fn controlled_fixture_terms(fixtures: &[Fixture], limit: usize) -> Vec<String> {
    let mut freq: BTreeMap<String, usize> = BTreeMap::new();
    for fixture in fixtures {
        let mut seen_in_fixture = BTreeSet::new();
        for token in &fixture.reference_tokens {
            if token.chars().count() < 6 {
                continue;
            }
            if !token.chars().any(|ch| ch.is_alphabetic()) {
                continue;
            }
            seen_in_fixture.insert(token.clone());
        }
        for token in seen_in_fixture {
            *freq.entry(token).or_default() += 1;
        }
    }

    let mut ranked: Vec<(String, usize)> = freq.into_iter().collect();
    ranked.sort_by(|(a, freq_a), (b, freq_b)| {
        freq_a
            .cmp(freq_b)
            .then_with(|| b.chars().count().cmp(&a.chars().count()))
            .then_with(|| a.cmp(b))
    });

    ranked.into_iter().take(limit).map(|(term, _)| term).collect()
}

fn write_kv(mut out: &File, key: &str, value: &str) -> Result<()> {
    writeln!(out, "{}\t{}", tsv_clean(key), tsv_clean(value))?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 6 {
        anyhow::bail!(
            "usage: {} MANIFEST WER_TSV TERM_HITS_TSV TERM_SOURCE_TSV LANGUAGE",
            args.first().map(String::as_str).unwrap_or("prompted-wer-probe")
        );
    }
    let manifest_path = PathBuf::from(&args[1]);
    let wer_tsv = PathBuf::from(&args[2]);
    let term_hits_tsv = PathBuf::from(&args[3]);
    let term_source_tsv = PathBuf::from(&args[4]);
    let language = args[5].as_str();
    let fixtures = read_manifest(&manifest_path)?;

    let runtime_prompt = stream_postprocess::whisper_initial_prompt();
    let runtime_terms = prompt_terms(runtime_prompt.as_deref());
    let runtime_reference_overlap = runtime_terms
        .iter()
        .filter(|term| fixtures.iter().any(|fixture| contains_term(&fixture.reference_norm, term)))
        .count();
    let force_controlled_prompt = env_truthy("CODESCRIBE_BENCH_FORCE_PROMPT");
    let runtime_prompt_term_limit = env_optional_usize("CODESCRIBE_BENCH_RUNTIME_PROMPT_TERM_LIMIT");
    let prompt_max_regression_pp = env_f64("CODESCRIBE_BENCH_PROMPT_MAX_WER_DELTA_PP", 5.0);

    let (source_kind, source_reason, selected_terms, prompt) =
        if runtime_prompt.is_some() && !runtime_terms.is_empty() {
            if let Some(limit) = runtime_prompt_term_limit
                && limit > 0
                && limit < runtime_terms.len()
            {
                let terms: Vec<String> = runtime_terms.iter().take(limit).cloned().collect();
                let prompt = stream_postprocess::build_whisper_initial_prompt(
                    &terms,
                    &[],
                    stream_postprocess::WHISPER_INITIAL_PROMPT_TOKEN_BUDGET,
                );
                (
                    "runtime_lexicon_trimmed",
                    "runtime protected/custom dictionary prompt trimmed by CODESCRIBE_BENCH_RUNTIME_PROMPT_TERM_LIMIT; order is the runtime prompt order",
                    terms,
                    prompt,
                )
            } else {
                (
                    "runtime_lexicon",
                    "runtime protected/custom dictionary prompt is enabled and nonempty",
                    runtime_terms.clone(),
                    runtime_prompt.clone(),
                )
            }
        } else if force_controlled_prompt {
            let terms = controlled_fixture_terms(&fixtures, 32);
            let prompt = stream_postprocess::build_whisper_initial_prompt(
                &terms,
                &[],
                stream_postprocess::WHISPER_INITIAL_PROMPT_TOKEN_BUDGET,
            );
            let reason = if runtime_prompt.is_none() || runtime_terms.is_empty() {
                "runtime protected/custom dictionary prompt is empty; generated deterministic fixture reference terms"
            } else {
                "runtime prompt exists but has zero overlap with references; generated deterministic fixture reference terms"
            };
            (
                "controlled_fixture_reference_terms",
                reason,
                terms,
                prompt,
            )
        } else {
            (
                "prompt_disabled",
                "runtime prompt is disabled or empty; prompted probe intentionally passes no initial_prompt",
                Vec::new(),
                None,
            )
        };

    let term_source_out = File::create(&term_source_tsv)?;
    writeln!(&term_source_out, "key\tvalue")?;
    write_kv(&term_source_out, "runtime_prompt_present", &runtime_prompt.is_some().to_string())?;
    write_kv(
        &term_source_out,
        "bench_force_prompt",
        &force_controlled_prompt.to_string(),
    )?;
    write_kv(
        &term_source_out,
        "runtime_terms_count",
        &runtime_terms.len().to_string(),
    )?;
    write_kv(
        &term_source_out,
        "runtime_reference_overlap",
        &runtime_reference_overlap.to_string(),
    )?;
    write_kv(
        &term_source_out,
        "runtime_prompt_term_limit",
        &runtime_prompt_term_limit
            .map(|limit| limit.to_string())
            .unwrap_or_else(|| "unset".to_string()),
    )?;
    write_kv(&term_source_out, "selected_source_kind", source_kind)?;
    write_kv(&term_source_out, "selected_terms_count", &selected_terms.len().to_string())?;
    write_kv(
        &term_source_out,
        "selected_terms",
        &selected_terms.join("; "),
    )?;
    write_kv(&term_source_out, "prompt_present", &prompt.is_some().to_string())?;
    write_kv(
        &term_source_out,
        "prompt_chars",
        &prompt.as_ref().map(|p| p.len()).unwrap_or(0).to_string(),
    )?;
    write_kv(
        &term_source_out,
        "prompt_preview",
        prompt.as_deref().unwrap_or(""),
    )?;
    write_kv(
        &term_source_out,
        "prompt_regression_guard_pp",
        &format!("{prompt_max_regression_pp:.3}"),
    )?;
    write_kv(&term_source_out, "source_reason", source_reason)?;

    whisper::init()?;

    let mut wer_out = File::create(&wer_tsv)?;
    writeln!(
        wer_out,
        "id\taudio_path\treference_path\tsource_kind\tunprompted_raw_wer\tprompted_raw_wer\tdelta_raw_wer_pp\tunprompted_post_wer\tprompted_post_wer\tdelta_post_wer_pp\tunprompted_raw_cer\tprompted_raw_cer\tunprompted_post_cer\tprompted_post_cer\tunprompted_raw_chars\tprompted_raw_chars"
    )?;
    let mut hits_out = File::create(&term_hits_tsv)?;
    writeln!(
        hits_out,
        "id\tterm\tsource_kind\treference_has_term\tunprompted_raw_hit\tprompted_raw_hit\tunprompted_post_hit\tprompted_post_hit"
    )?;
    let mut worst_raw_delta_pp = f32::NEG_INFINITY;
    let mut worst_post_delta_pp = f32::NEG_INFINITY;

    for fixture in fixtures {
        let (samples, sample_rate) = audio::load_audio_file(&fixture.audio_path)?;
        let unprompted = whisper::transcribe_with_segments(&samples, sample_rate, Some(language))?;
        let prompted = whisper::singleton::transcribe_with_segments_with_initial_prompt(
            &samples,
            sample_rate,
            Some(language),
            prompt.clone(),
        )?;
        let unprompted_post = stream_postprocess::apply_lexicon(&unprompted.text);
        let prompted_post = stream_postprocess::apply_lexicon(&prompted.text);

        let (unprompted_tokens, unprompted_norm) = normalize_for_eval(&unprompted.text);
        let (prompted_tokens, prompted_norm) = normalize_for_eval(&prompted.text);
        let (unprompted_post_tokens, unprompted_post_norm) = normalize_for_eval(&unprompted_post);
        let (prompted_post_tokens, prompted_post_norm) = normalize_for_eval(&prompted_post);

        let unprompted_raw_wer = word_error_rate(&fixture.reference_tokens, &unprompted_tokens);
        let prompted_raw_wer = word_error_rate(&fixture.reference_tokens, &prompted_tokens);
        let unprompted_post_wer =
            word_error_rate(&fixture.reference_tokens, &unprompted_post_tokens);
        let prompted_post_wer = word_error_rate(&fixture.reference_tokens, &prompted_post_tokens);
        let unprompted_raw_cer = char_error_rate(&fixture.reference_norm, &unprompted_norm);
        let prompted_raw_cer = char_error_rate(&fixture.reference_norm, &prompted_norm);
        let unprompted_post_cer = char_error_rate(&fixture.reference_norm, &unprompted_post_norm);
        let prompted_post_cer = char_error_rate(&fixture.reference_norm, &prompted_post_norm);
        let raw_delta_pp = (prompted_raw_wer - unprompted_raw_wer) * 100.0;
        let post_delta_pp = (prompted_post_wer - unprompted_post_wer) * 100.0;
        worst_raw_delta_pp = worst_raw_delta_pp.max(raw_delta_pp);
        worst_post_delta_pp = worst_post_delta_pp.max(post_delta_pp);

        writeln!(
            wer_out,
            "{}\t{}\t{}\t{}\t{:.6}\t{:.6}\t{:.3}\t{:.6}\t{:.6}\t{:.3}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{}\t{}",
            tsv_clean(&fixture.id),
            tsv_clean(&fixture.audio_path.display().to_string()),
            tsv_clean(&fixture.reference_path.display().to_string()),
            source_kind,
            unprompted_raw_wer,
            prompted_raw_wer,
            raw_delta_pp,
            unprompted_post_wer,
            prompted_post_wer,
            post_delta_pp,
            unprompted_raw_cer,
            prompted_raw_cer,
            unprompted_post_cer,
            prompted_post_cer,
            unprompted.text.chars().count(),
            prompted.text.chars().count()
        )?;

        for term in &selected_terms {
            if !contains_term(&fixture.reference_norm, term) {
                continue;
            }
            writeln!(
                hits_out,
                "{}\t{}\t{}\ttrue\t{}\t{}\t{}\t{}",
                tsv_clean(&fixture.id),
                tsv_clean(term),
                source_kind,
                contains_term(&unprompted_norm, term),
                contains_term(&prompted_norm, term),
                contains_term(&unprompted_post_norm, term),
                contains_term(&prompted_post_norm, term)
            )?;
        }
    }

    if prompt.is_some() {
        let worst_delta = worst_raw_delta_pp.max(worst_post_delta_pp);
        if f64::from(worst_delta) > prompt_max_regression_pp {
            anyhow::bail!(
                "prompted WER regression guard tripped: worst delta {worst_delta:.3} pp > allowed {prompt_max_regression_pp:.3} pp (source_kind={source_kind})"
            );
        }
    }

    Ok(())
}
EOF
}

run_qube_report() {
  rm -rf "$qube_out"
  log "running legacy qube-report WER control"
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

run_latency_probe() {
  : > "$latency_tsv"
  log "running scheduler-lane latency probe"
  (
    cd "$repo_root"
      BENCH_STT_LATENCY_MANIFEST="$manifest_tsv" \
      BENCH_STT_LATENCY_OUT="$latency_tsv" \
      BENCH_STT_LANGUAGE="$language" \
      CODESCRIBE_DISABLE_KEYCHAIN=1 \
      CODESCRIBE_NO_EMBED=1 \
      CODESCRIBE_MODEL_PATH="$model_path" \
      CARGO_TARGET_DIR="$repo_root/target" \
      cargo test -p codescribe-core bench_stt_scheduler_latency_probe_from_env -- --ignored --nocapture
  ) > "$latency_log" 2>&1
}

run_prompted_wer_probe() {
  write_wer_probe
  : > "$wer_tsv"
  : > "$term_hits_tsv"
  : > "$term_source_tsv"
  log "running prompted/unprompted WER probe"
  (
    CODESCRIBE_DISABLE_KEYCHAIN=1 CODESCRIBE_NO_EMBED=1 CODESCRIBE_MODEL_PATH="$model_path" CARGO_TARGET_DIR="$repo_root/target" \
      cargo run --quiet --manifest-path "$wer_probe_dir/Cargo.toml" -- \
        "$manifest_tsv" "$wer_tsv" "$term_hits_tsv" "$term_source_tsv" "$language"
  ) > "$wer_probe_log" 2>&1
}

write_summary_report() {
  local head_short repro fixture_source
  head_short="$(git -C "$repo_root" rev-parse --short=8 HEAD 2>/dev/null || printf 'unknown')"
  repro="scripts/bench-stt.sh --fixtures $fixture_mode --limit $fixture_limit --language $language"
  fixture_source="$(fixture_source_label "$manifest_tsv")"
  python3 - "$report_path" "$qube_out/report.json" "$manifest_tsv" "$latency_tsv" "$wer_tsv" "$term_hits_tsv" "$term_source_tsv" "$repro" "$repo_root" "$head_short" "$model_path" "$out_dir" "$fixture_source" <<'PY'
import csv
import json
import math
import sys
from pathlib import Path

report_path = Path(sys.argv[1])
qube_json = Path(sys.argv[2])
manifest_tsv = Path(sys.argv[3])
latency_tsv = Path(sys.argv[4])
wer_tsv = Path(sys.argv[5])
term_hits_tsv = Path(sys.argv[6])
term_source_tsv = Path(sys.argv[7])
repro = sys.argv[8]
repo_root = sys.argv[9]
head_short = sys.argv[10]
model_path = sys.argv[11]
out_dir = sys.argv[12]
fixture_source = sys.argv[13]

qube = json.loads(qube_json.read_text())
manifest = list(csv.DictReader(manifest_tsv.open(), delimiter="\t"))
latencies = {
    row["id"]: row
    for row in csv.DictReader(latency_tsv.open(), delimiter="\t")
}
wer_rows = list(csv.DictReader(wer_tsv.open(), delimiter="\t")) if wer_tsv.exists() else []
term_hit_rows = list(csv.DictReader(term_hits_tsv.open(), delimiter="\t")) if term_hits_tsv.exists() else []
term_source = {}
if term_source_tsv.exists():
    for row in csv.DictReader(term_source_tsv.open(), delimiter="\t"):
        term_source[row["key"]] = row["value"]

def pct(value):
    if value is None:
        return "n/a"
    return f"{value * 100:.2f}%"

def pp(value):
    if value is None:
        return "n/a"
    return f"{value:+.2f} pp"

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

def avg_key(rows, key):
    values = []
    for row in rows:
        value = row.get(key)
        if value in (None, "", "NA"):
            continue
        values.append(float(value))
    return sum(values) / len(values) if values else None

def hit_rate(rows, key):
    denom = len(rows)
    if denom == 0:
        return (0, 0, None)
    hits = sum(1 for row in rows if row.get(key) == "true")
    return (hits, denom, hits / denom)

def hit_fmt(rows, key):
    hits, denom, rate = hit_rate(rows, key)
    if rate is None:
        return "n/a"
    return f"{hits}/{denom} ({rate * 100:.1f}%)"

def row_by_id(rows):
    return {row.get("id"): row for row in rows}

summary = qube.get("summary", {})
entries = qube.get("entries", [])
first_values = [row.get("first_preview_ms") for row in latencies.values()]
final_values = [row.get("final_ms") for row in latencies.values()]
session_done_values = [row.get("session_done_ms") for row in latencies.values()]
wer_by_id = row_by_id(wer_rows)

unprompted_raw = avg_key(wer_rows, "unprompted_raw_wer")
prompted_raw = avg_key(wer_rows, "prompted_raw_wer")
unprompted_post = avg_key(wer_rows, "unprompted_post_wer")
prompted_post = avg_key(wer_rows, "prompted_post_wer")
raw_delta_pp = None if unprompted_raw is None or prompted_raw is None else (prompted_raw - unprompted_raw) * 100.0
post_delta_pp = None if unprompted_post is None or prompted_post is None else (prompted_post - unprompted_post) * 100.0

lines = []
lines.append("# Codescribe STT Real-Path Bench")
lines.append("")
lines.append("## Run Context")
lines.append("")
lines.append(f"- repo: `{repo_root}`")
lines.append(f"- head: `{head_short}`")
lines.append(f"- model: `{model_path}`")
lines.append(f"- output: `{out_dir}`")
lines.append(f"- fixtures: `{len(manifest)}`")
lines.append(f"- fixture_source: `{fixture_source}`")
lines.append("- legacy WER control: existing `qube-report` raw path")
lines.append("- scheduler latency: `transcription_session` -> `SttScheduler` lanes; model load is excluded")
lines.append("- prompted WER: explicit prompt-aware transcribe call vs unprompted control; with default OFF it passes no initial prompt")
lines.append("")
lines.append("## Repro Command")
lines.append("")
lines.append("```bash")
lines.append(repro)
lines.append("```")
lines.append("")
lines.append("## Term Source Proof")
lines.append("")
lines.append(f"- runtime prompt present: `{term_source.get('runtime_prompt_present', 'n/a')}`")
lines.append(f"- bench force prompt: `{term_source.get('bench_force_prompt', 'n/a')}`")
lines.append(f"- runtime terms: `{term_source.get('runtime_terms_count', 'n/a')}`")
lines.append(f"- runtime terms overlapping references: `{term_source.get('runtime_reference_overlap', 'n/a')}`")
lines.append(f"- selected source: `{term_source.get('selected_source_kind', 'n/a')}`")
lines.append(f"- selected terms: `{term_source.get('selected_terms_count', 'n/a')}`")
lines.append(f"- prompt chars: `{term_source.get('prompt_chars', 'n/a')}`")
lines.append(f"- prompt regression guard: `{term_source.get('prompt_regression_guard_pp', 'n/a')}` pp")
lines.append(f"- source reason: {term_source.get('source_reason', 'n/a')}")
lines.append("")
lines.append("## Metric Separation")
lines.append("")
lines.append("- Old metric: `qube-report` WER is retained as a legacy unprompted/control number.")
lines.append("- New WER metric: the probe runs the same fixtures twice, once through the regular unprompted path and once through the prompt-aware API.")
lines.append("- New latency metric: the probe drives `transcription_session`, so Live preview and Commit final pass travel through `SttScheduler`.")
lines.append("")
lines.append("## Summary Metrics")
lines.append("")
lines.append("| metric | value |")
lines.append("| --- | ---: |")
lines.append(f"| legacy qube raw WER | {pct(summary.get('avg_raw_wer'))} |")
lines.append(f"| legacy qube post WER | {pct(summary.get('avg_post_wer'))} |")
lines.append(f"| prompted probe unprompted raw WER | {pct(unprompted_raw)} |")
lines.append(f"| prompted probe prompted raw WER | {pct(prompted_raw)} |")
lines.append(f"| prompted raw delta | {pp(raw_delta_pp)} |")
lines.append(f"| prompted probe unprompted post WER | {pct(unprompted_post)} |")
lines.append(f"| prompted probe prompted post WER | {pct(prompted_post)} |")
lines.append(f"| prompted post delta | {pp(post_delta_pp)} |")
lines.append(f"| raw term hit-rate unprompted | {hit_fmt(term_hit_rows, 'unprompted_raw_hit')} |")
lines.append(f"| raw term hit-rate prompted | {hit_fmt(term_hit_rows, 'prompted_raw_hit')} |")
lines.append(f"| post term hit-rate unprompted | {hit_fmt(term_hit_rows, 'unprompted_post_hit')} |")
lines.append(f"| post term hit-rate prompted | {hit_fmt(term_hit_rows, 'prompted_post_hit')} |")
lines.append(f"| scheduler first preview avg/p95 | {ms(avg(first_values))} ms / {ms(p95(first_values))} ms |")
lines.append(f"| scheduler final avg/p95 | {ms(avg(final_values))} ms / {ms(p95(final_values))} ms |")
lines.append(f"| scheduler session done avg/p95 | {ms(avg(session_done_values))} ms / {ms(p95(session_done_values))} ms |")
lines.append("")
lines.append("## Baseline Reference")
lines.append("")
lines.append("| baseline | instrument | raw WER | post WER | first avg/p95 ms | final avg/p95 ms |")
lines.append("| --- | --- | ---: | ---: | ---: | ---: |")
lines.append("| W1-A run 2 | old direct/qube metric | 22.23% | 24.07% | 3230 / 4042 | 10821 / 36916 |")
lines.append("| W2-D run 1 | old direct/qube metric | 22.23% | 24.07% | 3706 / 4899 | 11955 / 41451 |")
lines.append("| W2-D run 2 | old direct/qube metric | 22.23% | 24.07% | 3304 / 5437 | 10963 / 36239 |")
lines.append("| this run | new scheduler/prompted metric | see above | see above | see above | see above |")
lines.append("")
lines.append("> Old direct-engine latency and new scheduler-lane latency are different metrics; deltas across that boundary are diagnostic only, not a regression claim.")
lines.append("")
lines.append("## Per-Fixture Metrics")
lines.append("")
lines.append("| file | legacy raw WER | legacy post WER | unprompted raw WER | prompted raw WER | raw delta | unprompted post WER | prompted post WER | post delta | first preview ms | final ms |")
lines.append("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
for entry in entries:
    row = latencies.get(entry.get("id"), {})
    wer_row = wer_by_id.get(entry.get("id"), {})
    metrics = entry.get("metrics", {})
    lines.append(
        "| {id} | {legacy_raw} | {legacy_post} | {un_raw} | {pr_raw} | {raw_delta} | {un_post} | {pr_post} | {post_delta} | {first} | {final} |".format(
            id=entry.get("id", "unknown"),
            legacy_raw=pct(metrics.get("raw_wer")),
            legacy_post=pct(metrics.get("post_wer")),
            un_raw=pct(float(wer_row["unprompted_raw_wer"])) if wer_row else "n/a",
            pr_raw=pct(float(wer_row["prompted_raw_wer"])) if wer_row else "n/a",
            raw_delta=pp(float(wer_row["delta_raw_wer_pp"])) if wer_row else "n/a",
            un_post=pct(float(wer_row["unprompted_post_wer"])) if wer_row else "n/a",
            pr_post=pct(float(wer_row["prompted_post_wer"])) if wer_row else "n/a",
            post_delta=pp(float(wer_row["delta_post_wer_pp"])) if wer_row else "n/a",
            first=ms(row.get("first_preview_ms")),
            final=ms(row.get("final_ms")),
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
lines.append("## Not Verified Here")
lines.append("")
lines.append("- This per-run report does not decide two-run convergence; the W2-F worker report compares run 1 and run 2.")
lines.append("- Scheduler latency excludes cold model load.")
lines.append("")
lines.append("## Artifacts")
lines.append("")
lines.append(f"- qube report JSON: `{qube_json}`")
lines.append(f"- scheduler latency TSV: `{latency_tsv}`")
lines.append(f"- prompted WER TSV: `{wer_tsv}`")
lines.append(f"- term hit-rate TSV: `{term_hits_tsv}`")
lines.append(f"- term source TSV: `{term_source_tsv}`")
lines.append(f"- fixture manifest: `{manifest_tsv}`")
lines.append("")
report_path.write_text("\n".join(lines) + "\n")

print("\n".join(lines[:44]))
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

if ! run_prompted_wer_probe; then
  write_honest_report "prompted/unprompted WER probe failed; see $wer_probe_log."
fi

if [[ ! -s "$wer_tsv" || ! -s "$term_hits_tsv" || ! -s "$term_source_tsv" ]]; then
  write_honest_report "prompted/unprompted WER probe produced incomplete artifacts."
fi

write_summary_report
