//! Long-file window truth — RED pack for the stt-live-first-v2 dispatch.
//!
//! Ground truth: the operator's three-way test recording (2026-08-10, 149.6 s,
//! eleven numbered sentences read against native Apple dictation and our
//! pipeline simultaneously). Measured failures these tests pin:
//!
//! - TAIL LOSS: `transcribe_long_with_language_segments` over the full file
//!   loses sentences 10–11 ("Zdanie 10. Zdanie 11. Zdajenie.") although the
//!   same seconds decode perfectly from a clip cut at 105 s — a fixed-step
//!   window starting inside mumbled speech (120 s) derails the whole window.
//! - HEAD DUPLICATION: the seam dedup is text-based (exact/fuzzy word match),
//!   and two windows decode the 5 s overlap differently, so sentence one is
//!   emitted twice ("…pułapek. Zdanie pierwsze. Zdanie drugie.").
//!
//! Run (requires local Whisper model + fixture):
//!   CODESCRIBE_E2E_STT=1 cargo test --test e2e_long_window_truth -- --nocapture
//!
//! Fixture resolution order (documented):
//!   1. $CODESCRIBE_LONG_WAV (explicit path)
//!   2. ~/.codescribe/data_assets/stt/threeway_test_full_149s.wav
//!
//! Missing fixture or model → skip with instructions, never a false green.

use std::path::PathBuf;

use codescribe::whisper::LocalWhisperEngine;
use serial_test::serial;

#[path = "support/e2e_stt_matrix.rs"]
mod e2e_stt_matrix;

use e2e_stt_matrix::{STT_OPT_IN_ENV, discover_local_whisper_model, skip_unless_opt_in};

const FIXTURE_NAME: &str = "threeway_test_full_149s.wav";

fn resolve_fixture() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("CODESCRIBE_LONG_WAV") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    let home = std::env::var("HOME").ok()?;
    let candidate = PathBuf::from(home)
        .join(".codescribe/data_assets/stt")
        .join(FIXTURE_NAME);
    candidate.is_file().then_some(candidate)
}

fn load_wav(path: &PathBuf) -> (Vec<f32>, u32) {
    let mut reader = hound::WavReader::open(path)
        .unwrap_or_else(|e| panic!("open fixture {}: {}", path.display(), e));
    let spec = reader.spec();
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .map(|s| s.expect("wav sample") as f32 / i16::MAX as f32)
            .collect(),
        (hound::SampleFormat::Int, 24 | 32) => reader
            .samples::<i32>()
            .map(|s| s.expect("wav sample") as f32 / i32::MAX as f32)
            .collect(),
        (hound::SampleFormat::Float, _) => reader
            .samples::<f32>()
            .map(|s| s.expect("wav sample"))
            .collect(),
        other => panic!("unsupported wav format: {other:?}"),
    };
    // Downmix to mono if needed — the engine expects one channel.
    let mono = if spec.channels > 1 {
        samples
            .chunks(spec.channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
            .collect()
    } else {
        samples
    };
    (mono, spec.sample_rate)
}

fn full_file_transcript() -> Option<String> {
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "e2e_long_window_truth",
        "Decodes a 149.6 s Polish recording with the local Whisper model.",
    ) {
        return None;
    }
    let Some(model) = discover_local_whisper_model() else {
        eprintln!("Skipping e2e_long_window_truth: no complete Whisper model found.");
        return None;
    };
    let Some(fixture) = resolve_fixture() else {
        eprintln!(
            "Skipping e2e_long_window_truth: fixture {FIXTURE_NAME} not found.\n  set CODESCRIBE_LONG_WAV=<path> or place it under ~/.codescribe/data_assets/stt/"
        );
        return None;
    };
    let (samples, sample_rate) = load_wav(&fixture);
    let mut engine = LocalWhisperEngine::new(&model.path).expect("load local whisper model");
    let transcript = engine
        .transcribe_long_with_language_segments(&samples, sample_rate, Some("pl"))
        .expect("long transcription");
    println!("segments={}", transcript.segments.len());
    for segment in &transcript.segments {
        println!(
            "segment [{:.2}, {:.2}] {:?}",
            segment.start_ts, segment.end_ts, segment.text
        );
    }
    println!("--- full-file transcript ---\n{}\n---", transcript.text);
    Some(transcript.text)
}

/// TAIL TRUTH: sentence 10 ("…przyjedzie z Warszawy…") and sentence 11
/// ("…jedenaste…") are audible in the fixture (clip cut at 105 s decodes them
/// verbatim — measured 2026-08-10); the full-file window loop must not lose
/// them. RED until VAD-aligned window boundaries land (cut w1-a).
#[test]
#[serial]
fn tail_sentences_survive_full_file_decode() {
    let Some(text) = full_file_transcript() else {
        return;
    };
    let lower = text.to_lowercase();
    assert!(
        lower.contains("warszawy"),
        "sentence 10 lost by the window loop (no 'Warszawy' in full-file decode)"
    );
    assert!(
        lower.contains("jedenaste"),
        "sentence 11 lost by the window loop (no 'jedenaste' in full-file decode)"
    );
}

/// SEAM TRUTH: the phrase "zdanie pierwsze" is spoken once; the seam between
/// window 1 and window 2 must not emit it twice. RED until the seam judge
/// dedups by segment time instead of by text (cut w1-c).
#[test]
#[serial]
fn head_is_not_duplicated_at_window_seams() {
    let Some(text) = full_file_transcript() else {
        return;
    };
    let lower = text.to_lowercase();
    let occurrences = lower.matches("zdanie pierwsze").count();
    assert!(
        occurrences <= 1,
        "'zdanie pierwsze' emitted {occurrences}x — window seam duplicated the head"
    );
}
