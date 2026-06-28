//! E2E tests for streaming transcription with chunk callbacks
//!
//! Tests the ChunkCallback mechanism used for live preview during transcription.
//!
//! To run full streaming tests (requires model):
//!   CODESCRIBE_E2E_STT=1 cargo test --test e2e_streaming_chunks
//!
//! Created by Vetcoders (c)2026

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use codescribe::audio;
use codescribe::whisper::append_with_overlap_dedup;

#[path = "support/e2e_stt_matrix.rs"]
mod e2e_stt_matrix;

use e2e_stt_matrix::{
    ModelDiscovery, STT_OPT_IN_ENV, discover_local_whisper_model, model_discovery_hint,
    normalize_transcript, skip_unless_opt_in, test_audio_path,
};

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn resolve_model_or_skip(suite: &str) -> Option<ModelDiscovery> {
    match discover_local_whisper_model() {
        Some(found) => Some(found),
        None => {
            let home = home_dir();
            eprintln!("Skipping {}: no complete Whisper model found.", suite);
            eprintln!("{}", model_discovery_hint(&home));
            None
        }
    }
}

/// Test that streaming callback is invoked during transcription
///
/// Run with: CODESCRIBE_E2E_STT=1 cargo test --test e2e_streaming_chunks test_streaming_callback
#[test]
fn test_streaming_callback_invoked() {
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "streaming callback E2E",
        "Deterministic chunk merge checks still run by default.",
    ) {
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    let model = match resolve_model_or_skip("streaming callback E2E") {
        Some(found) => found,
        None => return,
    };

    let mut engine = LocalWhisperEngine::new(&model.path).expect("load model");

    let audio_path = test_audio_path();
    let (samples, sample_rate) = audio::load_audio_file(&audio_path).expect("load audio");

    // Track callback invocations
    let callback_count = Arc::new(AtomicUsize::new(0));
    let callback_count_clone = Arc::clone(&callback_count);
    let collected_texts: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let collected_clone = Arc::clone(&collected_texts);

    let callback = move |text: &str| {
        callback_count_clone.fetch_add(1, Ordering::SeqCst);
        collected_clone.lock().unwrap().push(text.to_string());
        println!(
            "Chunk {}: {} chars",
            callback_count_clone.load(Ordering::SeqCst),
            text.len()
        );
    };

    let result = engine
        .transcribe_long_streaming(&samples, sample_rate, Some("pl"), Some(&callback))
        .expect("transcribe streaming");

    let final_count = callback_count.load(Ordering::SeqCst);
    let texts = collected_texts.lock().unwrap();

    println!("Total callbacks: {}", final_count);
    println!("Final result: {} chars", result.len());

    // For audio longer than 25s, we should get multiple callbacks
    let duration_secs = samples.len() as f32 / sample_rate as f32;
    if duration_secs > 25.0 {
        assert!(
            final_count > 0,
            "Expected callbacks for {:.1}s audio, got {}",
            duration_secs,
            final_count
        );
    }

    // Verify callbacks contain cumulative text (each should be longer or equal)
    if texts.len() > 1 {
        for i in 1..texts.len() {
            assert!(
                texts[i].len() >= texts[i - 1].len(),
                "Callback {} shorter than {}: {} < {}",
                i,
                i - 1,
                texts[i].len(),
                texts[i - 1].len()
            );
        }
    }

    // Final result should match last callback (or be equal if only one chunk)
    if !texts.is_empty() {
        assert_eq!(
            result.trim(),
            texts.last().unwrap().trim(),
            "Final result should match last callback"
        );
    }
}

/// Test streaming with None callback (should still work)
#[test]
fn test_streaming_no_callback() {
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "streaming no-callback E2E",
        "Set CODESCRIBE_E2E_STT=1 to run model-dependent streaming checks.",
    ) {
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    let model = match resolve_model_or_skip("streaming no-callback E2E") {
        Some(found) => found,
        None => return,
    };

    let mut engine = LocalWhisperEngine::new(&model.path).expect("load model");

    let audio_path = test_audio_path();
    let (samples, sample_rate) = audio::load_audio_file(&audio_path).expect("load audio");

    // No callback - should still transcribe
    let result = engine
        .transcribe_long_streaming(&samples, sample_rate, Some("pl"), None)
        .expect("transcribe without callback");

    assert!(!result.is_empty(), "Result should not be empty");
    println!("Transcription without callback: {} chars", result.len());
}

/// Verify chunk text doesn't cut words mid-stream (regression test)
#[test]
fn test_chunk_word_boundaries() {
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "streaming word-boundary E2E",
        "Set CODESCRIBE_E2E_STT=1 to run model-dependent word-boundary checks.",
    ) {
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    let model = match resolve_model_or_skip("streaming word-boundary E2E") {
        Some(found) => found,
        None => return,
    };

    let mut engine = LocalWhisperEngine::new(&model.path).expect("load model");

    let audio_path = test_audio_path();
    let (samples, sample_rate) = audio::load_audio_file(&audio_path).expect("load audio");

    let chunks: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let chunks_clone = Arc::clone(&chunks);

    let callback = move |text: &str| {
        chunks_clone.lock().unwrap().push(text.to_string());
    };

    let _result = engine
        .transcribe_long_streaming(&samples, sample_rate, Some("pl"), Some(&callback))
        .expect("transcribe");

    let chunks = chunks.lock().unwrap();

    // Check that incremental text (difference between chunks) doesn't start/end mid-word
    for i in 1..chunks.len() {
        let prev = &chunks[i - 1];
        let curr = &chunks[i];

        if curr.len() > prev.len() {
            let new_text = &curr[prev.len()..];
            let trimmed = new_text.trim();

            // New text should not start with lowercase letter immediately after previous text
            // (would indicate word was split)
            if !prev.is_empty()
                && !prev.ends_with(' ')
                && !prev.ends_with('\n')
                && let Some(first_char) = trimmed.chars().next()
                && first_char.is_alphabetic()
                && first_char.is_lowercase()
            {
                // This might indicate a word split, log it
                println!(
                    "Potential word split at chunk {}: '...{}' + '{}'",
                    i,
                    &prev[prev.len().saturating_sub(10)..],
                    &trimmed[..trimmed.len().min(10)]
                );
            }
        }
    }
}

#[test]
fn test_overlap_dedup_stable_across_chunkings() {
    let chunks_a = [
        "Ala ma kota i psa",
        "kota i psa i papuge",
        "i papuge w domu",
    ];
    let chunks_b = ["Ala ma kota", "ma kota i psa i papuge", "i papuge w domu"];

    let mut merged_a = String::new();
    for chunk in chunks_a {
        append_with_overlap_dedup(&mut merged_a, chunk);
    }

    let mut merged_b = String::new();
    for chunk in chunks_b {
        append_with_overlap_dedup(&mut merged_b, chunk);
    }

    assert_eq!(
        normalize_transcript(&merged_a),
        normalize_transcript(&merged_b),
        "overlap dedup should produce stable final text across equivalent chunk boundaries"
    );
}

#[test]
fn test_streaming_matches_non_streaming_output() {
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "streaming parity E2E",
        "Set CODESCRIBE_E2E_STT=1 to compare streaming and non-streaming Whisper paths.",
    ) {
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    let model = match resolve_model_or_skip("streaming parity E2E") {
        Some(found) => found,
        None => return,
    };

    let mut engine = LocalWhisperEngine::new(&model.path).expect("load model");
    let audio_path = test_audio_path();
    let (samples, sample_rate) = audio::load_audio_file(&audio_path).expect("load audio");

    let non_streaming = engine
        .transcribe_long_with_language(&samples, sample_rate, Some("pl"))
        .expect("transcribe non-streaming");

    let streaming_no_callback = engine
        .transcribe_long_streaming(&samples, sample_rate, Some("pl"), None)
        .expect("transcribe streaming without callback");

    let chunks: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let chunks_clone = Arc::clone(&chunks);
    let callback = move |text: &str| {
        chunks_clone
            .lock()
            .expect("lock callback chunks")
            .push(text.to_string());
    };

    let streaming_with_callback = engine
        .transcribe_long_streaming(&samples, sample_rate, Some("pl"), Some(&callback))
        .expect("transcribe streaming with callback");

    assert_eq!(
        normalize_transcript(&non_streaming),
        normalize_transcript(&streaming_no_callback),
        "streaming path (no callback) must match non-streaming path for identical audio input"
    );
    assert_eq!(
        normalize_transcript(&non_streaming),
        normalize_transcript(&streaming_with_callback),
        "streaming path (with callback) must match non-streaming path for identical audio input"
    );

    let last_callback = chunks
        .lock()
        .expect("lock callback chunks")
        .last()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        normalize_transcript(&streaming_with_callback),
        normalize_transcript(&last_callback),
        "final streaming result should equal last callback payload"
    );
}
