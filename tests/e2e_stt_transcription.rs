//! E2E test for STT transcription using local Whisper engine
//!
//! Adapted from examples/e2e_stt.rs to use test assets and proper test structure.
//!
//! To run (requires model):
//!   CODESCRIBE_E2E_STT=1 cargo test --test e2e_stt_transcription
//!
//! Created by M&K (c)2026 VetCoders

use std::path::PathBuf;

use codescribe::whisper::LocalWhisperEngine;

/// Path to synthetic test audio file
fn test_audio_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/1.fretka-Ziggy.mp3")
}

/// Find available Whisper model
fn find_model_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_candidates = [
        // Turbo model (faster)
        PathBuf::from(&home).join(".CodeScribe/models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from("../codescribe-core/models/whisper-large-v3-turbo-mlx-q8"),
        // Standard large-v3 model
        PathBuf::from(&home).join(".CodeScribe/models/whisper-large-v3-mlx-q8"),
        PathBuf::from("models/whisper-large-v3-mlx-q8"),
        PathBuf::from("../codescribe-core/models/whisper-large-v3-mlx-q8"),
    ];

    model_candidates
        .iter()
        .find(|p| p.join("tokenizer.json").exists())
        .cloned()
}

/// Full STT E2E test with local Whisper engine
///
/// Run with: CODESCRIBE_E2E_STT=1 cargo test --test e2e_stt_transcription
#[test]
fn e2e_stt_transcribe_test_audio() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping STT E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    let model_path = match find_model_path() {
        Some(p) => p,
        None => {
            eprintln!("No Whisper model found, skipping test");
            eprintln!("Expected model at ~/.CodeScribe/models/whisper-large-v3-turbo-mlx-q8");
            return;
        }
    };

    println!("═══════════════════════════════════════════════════════════");
    println!("  Local Whisper STT E2E Test");
    println!("═══════════════════════════════════════════════════════════");
    println!("  Model: {}", model_path.display());

    // Initialize engine
    println!("  Loading model...");
    let start = std::time::Instant::now();
    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");
    println!("  Model loaded in {:?}", start.elapsed());

    // Load and transcribe test audio
    let audio_path = test_audio_path();
    println!("  Audio: {}", audio_path.display());

    // Use Polish language (test audio is in Polish)
    let language = std::env::var("CODESCRIBE_E2E_LANG")
        .ok()
        .unwrap_or_else(|| "pl".to_string());
    println!("  Language: {}", language);
    println!("───────────────────────────────────────────────────────────");

    println!("  Transcribing...");
    let start = std::time::Instant::now();
    let text = engine
        .transcribe_file_with_language(&audio_path, Some(&language))
        .expect("transcribe");
    let elapsed = start.elapsed();

    println!("───────────────────────────────────────────────────────────");
    println!("  Transcription time: {:?}", elapsed);
    println!("  Characters: {}", text.len());
    println!("  Words: {}", text.split_whitespace().count());
    println!("═══════════════════════════════════════════════════════════");
    println!();
    println!("{}", text);
    println!();

    // Assertions
    assert!(!text.is_empty(), "Transcription should not be empty");
    assert!(
        text.len() > 20,
        "Transcription too short: {} chars",
        text.len()
    );
}

/// Test language detection
#[test]
fn e2e_stt_detect_language() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping language detection E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    let model_path = match find_model_path() {
        Some(p) => p,
        None => {
            eprintln!("No model found, skipping test");
            return;
        }
    };

    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");
    let audio_path = test_audio_path();

    println!("Detecting language for: {}", audio_path.display());
    let start = std::time::Instant::now();
    let detected = engine
        .detect_language_file(&audio_path)
        .expect("detect language");
    let elapsed = start.elapsed();

    println!("Detected language: {} (in {:?})", detected, elapsed);

    // Test audio is in Polish
    assert!(
        detected == "pl" || detected == "polish",
        "Expected Polish (pl), got: {}",
        detected
    );
}

/// Test that model initialization is idempotent
#[test]
fn e2e_stt_model_init_stable() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping model init E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    let model_path = match find_model_path() {
        Some(p) => p,
        None => {
            eprintln!("No model found, skipping test");
            return;
        }
    };

    // Initialize twice - should not panic
    let engine1 = LocalWhisperEngine::new(&model_path);
    assert!(engine1.is_ok(), "First init failed");

    let engine2 = LocalWhisperEngine::new(&model_path);
    assert!(engine2.is_ok(), "Second init failed");

    println!("Model initialization is stable (can be called multiple times)");
}
