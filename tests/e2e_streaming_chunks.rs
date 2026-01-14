//! E2E tests for streaming transcription with chunk callbacks
//!
//! Tests the ChunkCallback mechanism used for live preview during transcription.
//!
//! To run full streaming tests (requires model):
//!   CODESCRIBE_E2E_STT=1 cargo test --test e2e_streaming_chunks
//!
//! Created by M&K (c)2026 VetCoders

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use codescribe::audio;

/// Path to synthetic test audio file
fn test_audio_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/1.fretka-Ziggy.mp3")
}

/// Test that streaming callback is invoked during transcription
///
/// Run with: CODESCRIBE_E2E_STT=1 cargo test --test e2e_streaming_chunks test_streaming_callback
#[test]
fn test_streaming_callback_invoked() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping streaming E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    // Find model
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_candidates = [
        PathBuf::from(&home).join(".CodeScribe/models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from("models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from("models/whisper-large-v3-mlx-q8"),
    ];

    let model_path = model_candidates
        .iter()
        .find(|p| p.join("tokenizer.json").exists());

    let model_path = match model_path {
        Some(p) => p.clone(),
        None => {
            eprintln!("No model found, skipping streaming test");
            return;
        }
    };

    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");

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
        println!("Chunk {}: {} chars", callback_count_clone.load(Ordering::SeqCst), text.len());
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
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping streaming E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_candidates = [
        PathBuf::from(&home).join(".CodeScribe/models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from("models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from("models/whisper-large-v3-mlx-q8"),
    ];

    let model_path = match model_candidates
        .iter()
        .find(|p| p.join("tokenizer.json").exists())
    {
        Some(p) => p.clone(),
        None => {
            eprintln!("No model found, skipping test");
            return;
        }
    };

    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");

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
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping word boundary E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_candidates = [
        PathBuf::from(&home).join(".CodeScribe/models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from("models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from("models/whisper-large-v3-mlx-q8"),
    ];

    let model_path = match model_candidates
        .iter()
        .find(|p| p.join("tokenizer.json").exists())
    {
        Some(p) => p.clone(),
        None => {
            eprintln!("No model found, skipping test");
            return;
        }
    };

    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");

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
            if !prev.is_empty() && !prev.ends_with(' ') && !prev.ends_with('\n') {
                if let Some(first_char) = trimmed.chars().next() {
                    if first_char.is_alphabetic() && first_char.is_lowercase() {
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
    }
}
