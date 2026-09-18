//! E2E transcription primitives: audio → Whisper, transcript deltas, and VAD evidence
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
//! Created by Vetcoders (c)2026

use std::path::PathBuf;

use serial_test::serial;

use codescribe::whisper::LocalWhisperEngine;
use codescribe_core::audio::load_audio_file;
use codescribe_core::pipeline::contracts::{
    BACKSPACE, DeltaSink, FileTranscriptionOptions, TranscriptDelta,
};
use codescribe_core::pipeline::sinks::CollectorSink;
use codescribe_core::vad_api::{
    CHUNK_SIZE, Resampler, SAMPLE_RATE as VAD_SAMPLE_RATE, SileroVad, VadConfig,
};

// ═══════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════

/// Private STT fixtures live OUTSIDE the repo (real operator speech —
/// deprivatized twice). Resolution: `CODESCRIBE_DATA_ASSETS` →
/// `~/.codescribe/data_assets` → the gitignored in-repo drop dir.
fn assets_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CODESCRIBE_DATA_ASSETS") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        let local = PathBuf::from(home).join(".codescribe/data_assets");
        if local.is_dir() {
            return local;
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/data_assets")
}

/// Canonical test recordings with human reference transcriptions.
struct TestCase {
    name: &'static str,
    wav: &'static str,
    reference: &'static str,
    /// Key terms that MUST appear in transcription (case-insensitive).
    must_contain: &'static [&'static str],
}

const TEST_CASES: &[TestCase] = &[
    TestCase {
        name: "01 casual Polish (meta, loctree, Rust)",
        wav: "01_no-to-dobra.wav",
        reference: "01_no-to-dobra_human_transcription.txt",
        must_contain: &["codescribe", "transkrypcji", "leksykon"],
    },
    TestCase {
        name: "02 round 1: easy tech + vet",
        wav: "02_kubernetes-wymaga-konfiguracji.wav",
        reference: "02_kubernetes-wymaga-konfiguracji_human_transcription.txt",
        must_contain: &["kubernetes", "sql", "dawce"],
    },
    TestCase {
        name: "03 round 2: medium difficulty",
        wav: "03_algorytm-ma-zlozonosc.wav",
        reference: "03_algorytm-ma-zlozonosc_human_transcription.txt",
        must_contain: &["algorytm", "złożoność", "biopsj"],
    },
    TestCase {
        name: "04 round 3: hard mispronunciations",
        wav: "04_runda-3-czyli.wav",
        reference: "04_runda-3-czyli_human_transcription.txt",
        must_contain: &["tramadol", "kubernetes", "embeddingów"],
    },
];

fn find_model_path() -> Option<PathBuf> {
    codescribe_core::config::models::resolve_runtime_whisper_model_path(None).ok()
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

// ═══════════════════════════════════════════════════════════
// Stage 1: Raw Whisper on all 4 canonical recordings
// ═══════════════════════════════════════════════════════════

// Whisper-loading stage tests are #[serial]: since A1 all LocalWhisperEngine
// instances share one process-cached Metal Device, and concurrent decodes on
// separate engines corrupt each other's output (production serializes via the
// singleton mutex; these tests build their own engines).
#[test]
#[serial]
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
        let verdict = engine
            .transcribe_file_with_language(&audio, Some("pl"), FileTranscriptionOptions::default())
            .expect("transcribe");
        let elapsed = start.elapsed();
        let raw = verdict.text;

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
#[serial]
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
