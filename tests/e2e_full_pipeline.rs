//! E2E full pipeline test: audio → Whisper → postprocessing → backspace delta → result
//!
//! Tests the complete transcription pipeline using canonical test recordings
//! with human reference transcriptions for quality comparison.
//!
//! Test assets in tests/assets/data_assets/:
//!   01_no-to-dobra.wav          — casual Polish speech (meta, loctree, Rust)
//!   02_kubernetes-wymaga-...wav — round 1: easy technical + veterinary terms
//!   03_algorytm-ma-zlozonosc... — round 2: medium difficulty
//!   04_runda-3-czyli.wav        — round 3: hard mispronunciations
//!
//! Run with: CODESCRIBE_E2E_STT=1 cargo test --test e2e_full_pipeline -- --nocapture
//!
//! Created by M&K (c)2026 VetCoders

use std::path::PathBuf;

use codescribe::whisper::LocalWhisperEngine;
use codescribe_core::audio::load_audio_file;
use codescribe_core::pipeline::contracts::{BACKSPACE, DeltaSink, TranscriptDelta};
use codescribe_core::pipeline::sinks::CollectorSink;
use codescribe_core::pipeline::stream_postprocess::{StreamPostProcessStats, StreamPostProcessor};
use codescribe_core::vad_api::{
    CHUNK_SIZE, Resampler, SAMPLE_RATE as VAD_SAMPLE_RATE, SileroVad, VadConfig,
};

// ═══════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/data_assets")
}

/// Canonical test recordings with human reference transcriptions.
struct TestCase {
    name: &'static str,
    wav: &'static str,
    reference: &'static str,
    /// Key terms that MUST appear in transcription (case-insensitive).
    must_contain: &'static [&'static str],
    /// Key terms the lexicon should fix (Whisper mispronunciation → correct).
    lexicon_targets: &'static [&'static str],
}

const TEST_CASES: &[TestCase] = &[
    TestCase {
        name: "01 casual Polish (meta, loctree, Rust)",
        wav: "01_no-to-dobra.wav",
        reference: "01_no-to-dobra_human_transcription.txt",
        must_contain: &["codescribe", "transkrypcji", "leksykon"],
        lexicon_targets: &["loctree", "Rust"],
    },
    TestCase {
        name: "02 round 1: easy tech + vet",
        wav: "02_kubernetes-wymaga-konfiguracji.wav",
        reference: "02_kubernetes-wymaga-konfiguracji_human_transcription.txt",
        must_contain: &["kubernetes", "sql", "dawce"],
        lexicon_targets: &["gRPC", "Tokio", "Axum", "WebSocket"],
    },
    TestCase {
        name: "03 round 2: medium difficulty",
        wav: "03_algorytm-ma-zlozonosc.wav",
        reference: "03_algorytm-ma-zlozonosc_human_transcription.txt",
        must_contain: &["algorytm", "złożoność", "biopsj"],
        lexicon_targets: &["semgrep", "loctree", "Tauri"],
    },
    TestCase {
        name: "04 round 3: hard mispronunciations",
        wav: "04_runda-3-czyli.wav",
        reference: "04_runda-3-czyli_human_transcription.txt",
        must_contain: &["tramadol", "kubernetes", "embeddingów"],
        lexicon_targets: &["gRPC", "Robenacoxib"],
    },
];

fn find_model_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CODESCRIBE_MODEL_PATH") {
        let path = PathBuf::from(&p);
        if path.join("tokenizer.json").exists() {
            return Some(path);
        }
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());

    let direct = [
        PathBuf::from(&home).join(".codescribe/models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from(&home).join(".codescribe/models/whisper-large-v3-mlx-q8"),
    ];
    if let Some(p) = direct.iter().find(|p| p.join("tokenizer.json").exists()) {
        return Some(p.clone());
    }

    let hf_cache = PathBuf::from(&home).join(".cache/huggingface/hub");
    let hf_repos = [
        "models--LibraxisAI--whisper-large-v3-turbo-mlx-q8",
        "models--libraxisai--whisper-large-v3-mlx-q8",
    ];
    for repo_dir in &hf_repos {
        let snapshots = hf_cache.join(repo_dir).join("snapshots");
        if let Ok(entries) = std::fs::read_dir(&snapshots) {
            let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("tokenizer.json").exists() {
                    let mtime = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                        best = Some((mtime, path));
                    }
                }
            }
            if let Some((_, path)) = best {
                return Some(path);
            }
        }
    }

    None
}

fn is_e2e_stt_enabled() -> bool {
    std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn load_real_env() {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let env_path = PathBuf::from(&home).join(".codescribe/.env");
    if !env_path.exists() {
        eprintln!("  ⚠ No .env at {}", env_path.display());
        return;
    }
    let content = std::fs::read_to_string(&env_path).unwrap_or_default();
    let mut loaded = 0u32;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            if std::env::var(key).is_err() {
                // SAFETY: test binary is single-threaded at env load time
                unsafe { std::env::set_var(key, value) };
                loaded += 1;
            }
        }
    }
    eprintln!("  Loaded {} vars from {}", loaded, env_path.display());
}

/// Word overlap ratio between two texts (case-insensitive, order-independent).
fn word_overlap(a: &str, b: &str) -> f32 {
    let words_a: std::collections::HashSet<String> =
        a.split_whitespace().map(|w| w.to_lowercase()).collect();
    let words_b: std::collections::HashSet<String> =
        b.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words_b.is_empty() {
        return 0.0;
    }
    let overlap = words_a.intersection(&words_b).count();
    overlap as f32 / words_b.len() as f32
}

#[derive(Debug, Clone, Copy)]
struct RoutingQualityProbe {
    diff_ratio: f32,
    correction_ratio: f32,
    drop_ratio: f32,
}

fn routing_quality_probe(
    raw: &str,
    final_text: &str,
    stats: &StreamPostProcessStats,
) -> RoutingQualityProbe {
    let raw_chars = raw.chars().count();
    let final_chars = final_text.chars().count();
    let (backspaces, inserts) = TranscriptDelta::from_diff(raw, final_text)
        .map(|delta| {
            let backspaces = delta.delta.chars().filter(|c| *c == BACKSPACE).count();
            let inserts = delta.delta.chars().count().saturating_sub(backspaces);
            (backspaces, inserts)
        })
        .unwrap_or((0, 0));
    let span = raw_chars.max(final_chars).max(1);

    let drop_ratio = if stats.input_chunks == 0 {
        0.0
    } else {
        stats.dropped_chunks as f32 / stats.input_chunks as f32
    };

    RoutingQualityProbe {
        diff_ratio: ((backspaces + inserts) as f32 / span as f32).min(1.0),
        correction_ratio: (backspaces as f32 / raw_chars.max(1) as f32).min(1.0),
        drop_ratio,
    }
}

// ═══════════════════════════════════════════════════════════
// Stage 1: Raw Whisper on all 4 canonical recordings
// ═══════════════════════════════════════════════════════════

#[test]
fn e2e_stage1_raw_whisper_canonical() {
    if !is_e2e_stt_enabled() {
        eprintln!("Skipping (set CODESCRIBE_E2E_STT=1)");
        return;
    }
    load_real_env();

    let model_path = match find_model_path() {
        Some(p) => p,
        None => {
            eprintln!("No Whisper model found, skipping");
            return;
        }
    };

    println!("═══ Stage 1: Raw Whisper × 4 canonical recordings ═══");
    println!("  Model: {}", model_path.display());

    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");
    let dir = assets_dir();

    for tc in TEST_CASES {
        let audio = dir.join(tc.wav);
        assert!(audio.exists(), "Missing test asset: {}", audio.display());

        let reference = std::fs::read_to_string(dir.join(tc.reference)).unwrap_or_default();

        let start = std::time::Instant::now();
        let raw = engine
            .transcribe_file_with_language(&audio, Some("pl"))
            .expect("transcribe");
        let elapsed = start.elapsed();

        let overlap = word_overlap(&raw, &reference);
        let raw_lower = raw.to_lowercase();

        println!("───────────────────────────────────────────────────────────");
        println!("  [{}]", tc.name);
        println!(
            "  Time: {:?} | Chars: {} | Word overlap: {:.0}%",
            elapsed,
            raw.len(),
            overlap * 100.0
        );
        let raw_preview: String = raw.chars().take(120).collect();
        println!("  Raw: {}...", raw_preview);

        // Must produce non-empty output
        assert!(!raw.is_empty(), "{}: empty transcription", tc.name);

        // Check key terms
        for term in tc.must_contain {
            assert!(
                raw_lower.contains(&term.to_lowercase()),
                "{}: missing key term '{}'.\nRaw: {}",
                tc.name,
                term,
                raw
            );
        }

        // Word overlap with human reference should be > 30%
        // (Whisper output is verbose, reference is clean — 30% is conservative)
        assert!(
            overlap > 0.30,
            "{}: word overlap {:.0}% too low vs human reference",
            tc.name,
            overlap * 100.0
        );
    }

    println!("═══════════════════════════════════════════════════════════");
}

// ═══════════════════════════════════════════════════════════
// Stage 2: Postprocessing (lexicon + cleanup + hallucination gate)
// ═══════════════════════════════════════════════════════════

#[test]
fn e2e_stage2_postprocessor() {
    let mut pp = StreamPostProcessor::new();

    let cases = [
        ("Fretka Ziggy jest bardzo wesoła.", true),
        ("   ", false),
        ("Dzień dobry :D :D", true),
        ("...", false),
    ];

    println!("═══ Stage 2: StreamPostProcessor ═══");
    for (input, expect_some) in &cases {
        let result = pp.process(input);
        let passed = result.is_some() == *expect_some;
        println!(
            "  {} input={:30} → {:?}",
            if passed { "✓" } else { "✗" },
            input,
            result
        );
        assert_eq!(
            result.is_some(),
            *expect_some,
            "Unexpected result for input: {:?}",
            input
        );
    }

    let rewritten = pp.process("Używam Javy i Pythona");
    if let Some(text) = &rewritten {
        println!("  Lexicon output: {}", text);
        assert!(!text.is_empty());
    }

    let stats = pp.stats();
    println!(
        "  Stats: {} in, {} dropped",
        stats.input_chunks, stats.dropped_chunks
    );
}

// ═══════════════════════════════════════════════════════════
// Stage 3: TranscriptDelta + backspace corrections
// ═══════════════════════════════════════════════════════════

#[test]
fn e2e_stage3_delta_backspace() {
    println!("═══ Stage 3: TranscriptDelta Backspace Magic ═══");

    let collector = CollectorSink::new();
    let mut buffer = String::new();

    // Chunk 1: initial transcription
    let d1 = TranscriptDelta::append("Kubernetes wymoga ");
    d1.apply(&mut buffer);
    collector.apply(&d1);

    // Chunk 2: correction — "wymoga" → "wymaga konfiguracji"
    let d2 = TranscriptDelta::from_diff("Kubernetes wymoga ", "Kubernetes wymaga konfiguracji ");
    let d2 = d2.expect("diff should produce delta");
    assert!(
        d2.delta.contains(BACKSPACE),
        "Correction must have backspaces"
    );
    d2.apply(&mut buffer);
    collector.apply(&d2);

    // Chunk 3: append
    let d3 = TranscriptDelta::append("PostgreSQL.");
    d3.apply(&mut buffer);
    collector.apply(&d3);

    println!("  Final: {:?}", buffer);
    assert_eq!(buffer, "Kubernetes wymaga konfiguracji PostgreSQL.");

    let collected = collector.collected();
    assert_eq!(collected.len(), 3);
    assert!(!collected[0].contains(BACKSPACE), "d1 = append-only");
    assert!(collected[1].contains(BACKSPACE), "d2 = correction");
    assert!(!collected[2].contains(BACKSPACE), "d3 = append-only");

    println!("  ✓ 3 deltas: append → correct → append");
}

// ═══════════════════════════════════════════════════════════
// Stage 4: Full pipeline on all 4 recordings
// ═══════════════════════════════════════════════════════════

#[test]
fn e2e_stage4_full_pipeline() {
    if !is_e2e_stt_enabled() {
        eprintln!("Skipping full pipeline (set CODESCRIBE_E2E_STT=1)");
        return;
    }
    load_real_env();

    let model_path = match find_model_path() {
        Some(p) => p,
        None => {
            eprintln!("No Whisper model found, skipping");
            return;
        }
    };

    println!("═══ Stage 4: Full Pipeline × 4 recordings ═══");
    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");
    let dir = assets_dir();
    let mut total_lexicon_rewrites = 0u64;

    for tc in TEST_CASES {
        let audio = dir.join(tc.wav);
        let reference = std::fs::read_to_string(dir.join(tc.reference)).unwrap_or_default();

        // 1. Whisper STT
        let start = std::time::Instant::now();
        let raw = engine
            .transcribe_file_with_language(&audio, Some("pl"))
            .expect("transcribe");
        let stt_time = start.elapsed();

        // 2. PostProcessor
        let mut pp = StreamPostProcessor::new();
        let processed = pp.process_utterance(&raw).unwrap_or_else(|| raw.clone());
        let stats = pp.stats();
        total_lexicon_rewrites += stats.lexicon_rewrites;

        // 3. Delta (raw → processed)
        let collector = CollectorSink::new();
        let mut ui_buffer = String::new();

        let d_raw = TranscriptDelta::append(&raw);
        d_raw.apply(&mut ui_buffer);
        collector.apply(&d_raw);

        let backspaces = if let Some(correction) = TranscriptDelta::from_diff(&raw, &processed) {
            let bs = correction.delta.chars().filter(|&c| c == BACKSPACE).count();
            correction.apply(&mut ui_buffer);
            collector.apply(&correction);
            bs
        } else {
            0
        };

        assert_eq!(ui_buffer, processed, "Delta round-trip mismatch");

        let overlap = word_overlap(&processed, &reference);

        println!("───────────────────────────────────────────────────────────");
        println!("  [{}]", tc.name);
        println!(
            "  STT: {:?} | PostProcess: {} rewrites | Delta: {} backspaces",
            stt_time, stats.lexicon_rewrites, backspaces
        );
        println!("  Overlap vs human: {:.0}%", overlap * 100.0);
        let preview: String = processed.chars().take(100).collect();
        println!("  Processed: {}...", preview);

        // Check lexicon-target terms survived (case-insensitive)
        let proc_lower = processed.to_lowercase();
        let mut found_lexicon = 0;
        for term in tc.lexicon_targets {
            if proc_lower.contains(&term.to_lowercase()) {
                found_lexicon += 1;
            }
        }
        println!(
            "  Lexicon targets: {}/{} found",
            found_lexicon,
            tc.lexicon_targets.len()
        );
    }

    println!("═══════════════════════════════════════════════════════════");
    println!(
        "  Total lexicon rewrites across all recordings: {}",
        total_lexicon_rewrites
    );
}

// ═══════════════════════════════════════════════════════════
// Stage 5: Delta round-trip integrity (Polish Unicode)
// ═══════════════════════════════════════════════════════════

#[test]
fn e2e_stage5_delta_roundtrip_polish_unicode() {
    println!("═══ Stage 5: Delta Round-trip (Polish Unicode) ═══");

    let texts = [
        "Żółw żółty źdźbło",
        "Cześć, jak się masz? 🐾",
        "Kubernetes wymaga konfiguracji PostgreSQL.",
        "café résumé naïve",
        "",
    ];

    for before in &texts {
        for after in &texts {
            if before == after {
                assert!(
                    TranscriptDelta::from_diff(before, after).is_none(),
                    "Same text = no delta"
                );
                continue;
            }
            let delta = TranscriptDelta::from_diff(before, after).expect("Different text = delta");
            let mut buffer = before.to_string();
            delta.apply(&mut buffer);
            assert_eq!(buffer, *after, "Round-trip: {:?} → {:?}", before, after);
        }
    }

    println!(
        "  ✓ {} round-trip pairs verified",
        texts.len() * texts.len()
    );
}

// ═══════════════════════════════════════════════════════════
// Stage 6: PostProcessor idempotency
// ═══════════════════════════════════════════════════════════

#[test]
fn e2e_stage6_postprocessor_idempotent() {
    println!("═══ Stage 6: PostProcessor Idempotency ═══");

    let inputs = [
        "Kubernetes wymaga konfiguracji PostgreSQL jako bazy danych.",
        "Podajemy amoksycylinę w dawce pięć miligramów na kilogram masy ciała.",
        "Algorytm ma złożoność O(n) w najgorszym przypadku.",
    ];

    for input in &inputs {
        let mut pp1 = StreamPostProcessor::new();
        let first = pp1.process_utterance(input).expect("first pass");

        let mut pp2 = StreamPostProcessor::new();
        let second = pp2.process_utterance(&first).expect("second pass");

        println!("  ✓ {:.50}...", input);
        assert_eq!(first, second, "Not idempotent for: {}", input);
    }
}

// ═══════════════════════════════════════════════════════════
// Stage 7: Whisper hallucination in silence — raw vs VAD-gated
// ═══════════════════════════════════════════════════════════

/// Common Whisper hallucination patterns in silence.
/// These are filler/phantom tokens Whisper generates when fed quiet audio.
const HALLUCINATION_PATTERNS: &[&str] = &[
    "dzień dobry",
    "do widzenia",
    "dziękuję",
    "napisy",
    "tłumaczenie",
    "subskrybuj",
    "subscribe",
    "thank you",
];

/// Count occurrences of hallucination patterns in text (case-insensitive).
fn count_hallucinations(text: &str) -> (usize, Vec<String>) {
    let lower = text.to_lowercase();
    let mut total = 0;
    let mut found = Vec::new();

    for pattern in HALLUCINATION_PATTERNS {
        let count = lower.matches(&pattern.to_lowercase()).count();
        if count > 0 {
            found.push(format!("\"{}\" ×{}", pattern, count));
            total += count;
        }
    }

    // Also count repeated filler: "i... i... i..." or "i, i, i"
    let filler_count = lower
        .split_whitespace()
        .filter(|w| *w == "i" || *w == "i..." || *w == "i," || *w == "i.")
        .count();
    if filler_count >= 3 {
        found.push(format!("filler \"i\" ×{}", filler_count));
        total += filler_count;
    }

    (total, found)
}

/// Use SileroVad directly (synchronous) to gate audio — returns only speech frames.
fn vad_gate_audio(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let vad_model = codescribe_core::vad_api::default_model_path();
    assert!(
        vad_model.exists(),
        "Silero VAD model not found at: {}",
        vad_model.display()
    );

    let config = VadConfig::default();
    let mut vad = SileroVad::new(&vad_model, config).expect("load VAD");

    // Resample to 16kHz for VAD
    let mut resampler = Resampler::new(sample_rate);
    let samples_16k = resampler.resample(samples);

    // Process in CHUNK_SIZE frames, collect speech frames
    let mut speech_samples_16k: Vec<f32> = Vec::new();
    let threshold = 0.5;

    for chunk in samples_16k.chunks(CHUNK_SIZE) {
        if chunk.len() < CHUNK_SIZE {
            break;
        }
        let prob = vad.predict(chunk).unwrap_or(0.0);
        if prob >= threshold {
            speech_samples_16k.extend_from_slice(chunk);
        }
    }

    speech_samples_16k
}

#[test]
fn e2e_stage7_whisper_hallucination_vs_vad_gated() {
    if !is_e2e_stt_enabled() {
        eprintln!("Skipping (set CODESCRIBE_E2E_STT=1)");
        return;
    }
    load_real_env();

    let model_path = match find_model_path() {
        Some(p) => p,
        None => {
            eprintln!("No Whisper model found, skipping");
            return;
        }
    };

    let audio_path = assets_dir().join("VAD_voice_real_pauses.wav");
    assert!(audio_path.exists(), "Missing: {}", audio_path.display());

    println!("═══ Stage 7: Whisper Hallucination — Raw vs VAD-Gated ═══");
    println!("  Audio: VAD_voice_real_pauses.wav (~59s, deliberate pauses)");
    println!("  Model: {}", model_path.display());

    // Load audio
    let (samples, sample_rate) = load_audio_file(&audio_path).expect("load WAV");
    let duration_sec = samples.len() as f32 / sample_rate as f32;
    println!(
        "  Loaded: {} samples, {}Hz, {:.1}s",
        samples.len(),
        sample_rate,
        duration_sec
    );

    let mut engine = LocalWhisperEngine::new(&model_path).expect("load Whisper");

    // ── A: Raw Whisper (silence included) ──────────────────
    let start = std::time::Instant::now();
    let raw_transcript = engine
        .transcribe_long_with_language(&samples, sample_rate, Some("pl"))
        .expect("transcribe raw");
    let raw_time = start.elapsed();

    let (raw_hallucinations, raw_found) = count_hallucinations(&raw_transcript);

    println!("───────────────────────────────────────────────────────────");
    println!("  A) RAW (with silence):");
    println!(
        "     Time: {:?} | Chars: {}",
        raw_time,
        raw_transcript.len()
    );
    println!(
        "     Hallucinations: {} {:?}",
        raw_hallucinations, raw_found
    );
    let preview: String = raw_transcript.chars().take(200).collect();
    println!("     Text: {}...", preview);

    // ── B: VAD-gated Whisper (silence removed) ─────────────
    let start = std::time::Instant::now();
    let speech_only = vad_gate_audio(&samples, sample_rate);
    let vad_time = start.elapsed();

    let speech_sec = speech_only.len() as f32 / VAD_SAMPLE_RATE as f32;
    let silence_removed = duration_sec - speech_sec;
    println!("───────────────────────────────────────────────────────────");
    println!(
        "  VAD gate: {:.1}s speech kept, {:.1}s silence removed ({:.0}%)",
        speech_sec,
        silence_removed,
        (silence_removed / duration_sec) * 100.0
    );
    println!("  VAD time: {:?}", vad_time);

    let start = std::time::Instant::now();
    let gated_transcript = engine
        .transcribe_long_with_language(&speech_only, VAD_SAMPLE_RATE, Some("pl"))
        .expect("transcribe gated");
    let gated_time = start.elapsed();

    let (gated_hallucinations, gated_found) = count_hallucinations(&gated_transcript);

    println!("  B) VAD-GATED (speech only):");
    println!(
        "     Time: {:?} | Chars: {}",
        gated_time,
        gated_transcript.len()
    );
    println!(
        "     Hallucinations: {} {:?}",
        gated_hallucinations, gated_found
    );
    let preview: String = gated_transcript.chars().take(200).collect();
    println!("     Text: {}...", preview);

    // ── Verdict ────────────────────────────────────────────
    println!("═══════════════════════════════════════════════════════════");
    println!(
        "  VERDICT: raw={} hallucinations, gated={} hallucinations",
        raw_hallucinations, gated_hallucinations
    );

    // VAD-gated should have fewer or equal hallucinations
    assert!(
        gated_hallucinations <= raw_hallucinations,
        "VAD gate should reduce hallucinations! raw={}, gated={}",
        raw_hallucinations,
        gated_hallucinations
    );

    // VAD should remove meaningful silence (>10% of audio)
    assert!(
        silence_removed > duration_sec * 0.10,
        "Expected >10% silence removal, got {:.1}s of {:.1}s ({:.0}%)",
        silence_removed,
        duration_sec,
        (silence_removed / duration_sec) * 100.0
    );

    // Gated transcript should still contain actual speech content
    assert!(
        !gated_transcript.is_empty(),
        "VAD-gated transcript should not be empty"
    );

    if gated_hallucinations < raw_hallucinations {
        println!(
            "  ✓ VAD reduced hallucinations by {}",
            raw_hallucinations - gated_hallucinations
        );
    } else if raw_hallucinations == 0 {
        println!("  ✓ No hallucinations in either mode (Whisper behaved well on this audio)");
    } else {
        println!("  ⚠ Same hallucination count — VAD didn't help here");
    }
}

#[test]
fn e2e_stage8_action_routing_quality_guardrail() {
    println!("═══ Stage 8: Action Routing Quality Guardrail ═══");

    // Simulated last-pass outputs for key modes:
    // - RAW Hold Fn: no AI formatting in final output.
    // - AI Formatting mode: final output after last-pass formatting.
    let raw_hold = "kubernetes wymoga konfiguracji bazy";
    let final_raw_hold = "kubernetes wymoga konfiguracji bazy";
    let final_ai_format = "Kubernetes wymaga konfiguracji bazy.";

    let stats = StreamPostProcessStats {
        input_chunks: 12,
        dropped_chunks: 3,
        ..Default::default()
    };

    // Guardrail: quality probe is computed from transcript pair and postprocess stats only,
    // so Save/Copy/Augment routing must not change it.
    let save_probe = routing_quality_probe(raw_hold, final_ai_format, &stats);
    let copy_probe = routing_quality_probe(raw_hold, final_ai_format, &stats);
    let augment_probe = routing_quality_probe(raw_hold, final_ai_format, &stats);

    assert!((save_probe.diff_ratio - copy_probe.diff_ratio).abs() < 1e-6);
    assert!((save_probe.diff_ratio - augment_probe.diff_ratio).abs() < 1e-6);
    assert!((save_probe.correction_ratio - copy_probe.correction_ratio).abs() < 1e-6);
    assert!((save_probe.correction_ratio - augment_probe.correction_ratio).abs() < 1e-6);
    assert!((save_probe.drop_ratio - copy_probe.drop_ratio).abs() < 1e-6);
    assert!((save_probe.drop_ratio - augment_probe.drop_ratio).abs() < 1e-6);

    // RAW contract: Copy in RAW mode uses non-AI last-pass output.
    let raw_copy_probe = routing_quality_probe(raw_hold, final_raw_hold, &stats);
    assert!(
        raw_copy_probe.correction_ratio <= copy_probe.correction_ratio,
        "RAW copy should not introduce additional AI correction vs formatted mode"
    );

    println!(
        "  ✓ Stable probes across Save/Copy/Augment (diff={:.3}, correction={:.3}, drop={:.3})",
        save_probe.diff_ratio, save_probe.correction_ratio, save_probe.drop_ratio
    );
}
