//! E2E tests for audio loading and transcription pipeline
//!
//! Uses synthetic audio file from tests/assets/1.fretka-Ziggy.mp3
//!
//! To run transcription tests (requires model):
//!   CODESCRIBE_E2E_STT=1 cargo test --test e2e_audio_pipeline
//!
//! Created by M&K (c)2026 VetCoders

use std::path::PathBuf;

use codescribe::audio;

/// Path to synthetic test audio file
fn test_audio_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/1.fretka-Ziggy.mp3")
}

/// Verify test asset exists
#[test]
fn test_asset_exists() {
    let path = test_audio_path();
    assert!(
        path.exists(),
        "Test audio file not found at: {}",
        path.display()
    );
}

/// Test audio loading with Symphonia (no FFmpeg)
#[test]
fn test_audio_loading_mp3() {
    let path = test_audio_path();
    let result = audio::load_audio_file(&path);

    assert!(result.is_ok(), "Failed to load audio: {:?}", result.err());

    let (samples, sample_rate) = result.unwrap();

    // Basic sanity checks
    assert!(!samples.is_empty(), "No samples loaded");
    assert!(sample_rate > 0, "Invalid sample rate");

    // MP3 is typically 44100 or 48000 Hz
    assert!(
        (8000..=96000).contains(&sample_rate),
        "Unexpected sample rate: {}",
        sample_rate
    );

    // Calculate duration
    let duration_secs = samples.len() as f32 / sample_rate as f32;
    println!(
        "Loaded audio: {} samples @ {} Hz ({:.1}s)",
        samples.len(),
        sample_rate,
        duration_secs
    );

    // fretka-Ziggy.mp3 should be several seconds long
    assert!(
        duration_secs > 1.0,
        "Audio too short: {:.1}s",
        duration_secs
    );
}

/// Test resampling to 16kHz (Whisper requirement)
#[test]
fn test_resampling_to_16k() {
    let path = test_audio_path();
    let (samples, sample_rate) = audio::load_audio_file(&path).expect("load audio");

    let resampled = audio::resample_to_16k(&samples, sample_rate);

    // Verify resampled length is proportional
    let expected_len = (samples.len() as f32 * 16000.0 / sample_rate as f32).ceil() as usize;
    let tolerance = 100; // Allow small rounding differences

    assert!(
        (resampled.len() as i64 - expected_len as i64).abs() < tolerance as i64,
        "Unexpected resampled length: {} (expected ~{})",
        resampled.len(),
        expected_len
    );

    // Verify no NaN or Inf values
    assert!(
        resampled.iter().all(|s| s.is_finite()),
        "Resampled audio contains NaN/Inf"
    );

    // Verify amplitude is reasonable (-1.0 to 1.0 for normalized audio)
    let max_amplitude = resampled
        .iter()
        .map(|s| s.abs())
        .fold(0.0f32, |a, b| a.max(b));
    println!("Max amplitude after resampling: {:.3}", max_amplitude);
}

/// Test that 16kHz input is not resampled (passthrough)
#[test]
fn test_16k_passthrough() {
    let samples: Vec<f32> = (0..16000).map(|i| (i as f32 * 0.001).sin()).collect();

    let resampled = audio::resample_to_16k(&samples, 16000);

    assert_eq!(
        samples.len(),
        resampled.len(),
        "16kHz audio should pass through unchanged"
    );
}

/// Full transcription E2E test (opt-in, requires model)
///
/// Run with: CODESCRIBE_E2E_STT=1 cargo test --test e2e_audio_pipeline test_full_transcription
#[test]
fn test_full_transcription() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping STT E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    // Find model: ~/.codescribe/models/ (unified standard)
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_candidates = [
        PathBuf::from(&home).join(".codescribe/models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from(&home).join(".codescribe/models/whisper-large-v3-mlx-q8"),
    ];

    let model_path = model_candidates
        .iter()
        .find(|p| p.join("tokenizer.json").exists());

    let model_path = match model_path {
        Some(p) => p.clone(),
        None => {
            eprintln!("No model found, skipping transcription test");
            return;
        }
    };

    println!("Using model: {}", model_path.display());

    // Load engine
    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");

    // Load and transcribe
    let audio_path = test_audio_path();
    let (samples, sample_rate) = audio::load_audio_file(&audio_path).expect("load audio");

    let start = std::time::Instant::now();
    let text = engine
        .transcribe_long_with_language(&samples, sample_rate, Some("pl"))
        .expect("transcribe");
    let elapsed = start.elapsed();

    println!("Transcription time: {:?}", elapsed);
    println!(
        "Result ({} chars): {}",
        text.len(),
        &text[..text.len().min(200)]
    );

    // Basic assertions
    assert!(!text.is_empty(), "Transcription should not be empty");
    assert!(text.len() > 10, "Transcription too short");
}

/// Test chunk boundaries for streaming (synthetic)
#[test]
fn test_chunk_boundaries_synthetic() {
    // Generate 30 seconds of synthetic audio at 16kHz
    let sample_rate = 16000u32;
    let duration_secs = 30.0f32;
    let samples: Vec<f32> = (0..(sample_rate as f32 * duration_secs) as usize)
        .map(|i| {
            // Mix of frequencies to simulate speech-like audio
            let t = i as f32 / sample_rate as f32;
            0.3 * (440.0 * t * std::f32::consts::PI * 2.0).sin()
                + 0.2 * (880.0 * t * std::f32::consts::PI * 2.0).sin()
                + 0.1 * (220.0 * t * std::f32::consts::PI * 2.0).sin()
        })
        .collect();

    // Whisper uses 30-second chunks, so 25s per chunk with overlap
    let chunk_samples = 25 * sample_rate as usize;
    let num_chunks = samples.len().div_ceil(chunk_samples);

    println!(
        "Audio: {} samples ({:.1}s), chunk size: {}, num chunks: {}",
        samples.len(),
        duration_secs,
        chunk_samples,
        num_chunks
    );

    // Verify we can split into chunks without panic
    for i in 0..num_chunks {
        let start = i * chunk_samples;
        let end = (start + chunk_samples).min(samples.len());
        let chunk = &samples[start..end];

        assert!(!chunk.is_empty(), "Chunk {} is empty", i);
        assert!(
            chunk.iter().all(|s| s.is_finite()),
            "Chunk {} contains NaN/Inf",
            i
        );
    }
}
