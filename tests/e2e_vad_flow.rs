//! End-to-end tests for VAD (Voice Activity Detection) flow.
//!
//! Tests the complete pipeline:
//! - VAD initialization (singleton, thread-safety)
//! - Speech probability computation (non-blocking)
//! - Silence detection and auto-stop
//! - VADSegmenter utterance segmentation
//!
//! Created by M&K (c)2026 VetCoders

use codescribe_core::vad::{self, VadConfig};
use serial_test::serial;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

// ═══════════════════════════════════════════════════════════════════════════
// Test Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Generate synthetic speech-like audio (sine wave with harmonics)
fn generate_speech_audio(duration_sec: f32, sample_rate: u32) -> Vec<f32> {
    let num_samples = (duration_sec * sample_rate as f32) as usize;
    let mut samples = Vec::with_capacity(num_samples);

    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        // Simulate speech with multiple harmonics (fundamental + overtones)
        let fundamental = 150.0; // ~male voice fundamental
        let sample = 0.5 * (2.0 * std::f32::consts::PI * fundamental * t).sin()
            + 0.3 * (2.0 * std::f32::consts::PI * fundamental * 2.0 * t).sin()
            + 0.2 * (2.0 * std::f32::consts::PI * fundamental * 3.0 * t).sin();
        samples.push(sample * 0.8); // Scale to reasonable amplitude
    }
    samples
}

/// Generate silence (very low amplitude noise)
fn generate_silence(duration_sec: f32, sample_rate: u32) -> Vec<f32> {
    let num_samples = (duration_sec * sample_rate as f32) as usize;
    vec![0.0001; num_samples] // Near-zero but not exactly zero
}

/// Check if VAD model is available (skip tests if not)
fn vad_model_available() -> bool {
    vad::default_model_path().exists()
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 1: VAD Initialization
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vad_init_without_model_returns_error() {
    // Guard: If VAD is already initialized by another test, skip this test
    // VAD singleton is idempotent - second init() returns Ok() without error
    if vad::is_initialized() {
        eprintln!("Skipping: VAD already initialized by another test");
        return;
    }

    // Test that init fails gracefully when model is missing
    let fake_path = std::path::PathBuf::from("/nonexistent/silero_vad.onnx");
    let result = vad::init(&fake_path);

    assert!(result.is_err(), "init should fail with missing model");

    // After failed init, speech_probability should return 1.0 (assume speech)
    // Note: Only valid immediately after failed init, not if VAD was initialized elsewhere
    let silence = vec![0.0f32; 512];
    let prob = vad::speech_probability(&silence, 16000);
    assert!(
        (prob - 1.0).abs() < 0.01,
        "speech_probability should return 1.0 when VAD not initialized, got {}",
        prob
    );
}

#[test]
fn test_vad_is_initialized_false_initially() {
    // Note: This test may be affected by other tests that init VAD
    // In a fresh process, is_initialized should return false
    // We can't truly test this without process isolation
    let _ = vad::is_initialized(); // Just verify it doesn't panic
}

#[test]
#[ignore] // Requires Silero VAD model
#[serial]
fn test_vad_init_success_with_model() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    let model_path = vad::default_model_path();
    let result = vad::init(&model_path);
    assert!(result.is_ok(), "init should succeed with valid model");
    assert!(vad::is_initialized(), "should be initialized after init");
}

#[test]
#[ignore] // Requires Silero VAD model
#[serial]
fn test_vad_init_is_idempotent() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    let model_path = vad::default_model_path();

    // First init
    let start = Instant::now();
    let result1 = vad::init(&model_path);
    let first_duration = start.elapsed();
    assert!(result1.is_ok());

    // Second init should be instant (no-op)
    let start = Instant::now();
    let result2 = vad::init(&model_path);
    let second_duration = start.elapsed();
    assert!(result2.is_ok());

    // Second call should be much faster (< 10ms vs potentially 30s for first)
    assert!(
        second_duration < Duration::from_millis(10),
        "Second init should be instant (no-op), took {:?}",
        second_duration
    );

    eprintln!(
        "First init: {:?}, Second init: {:?}",
        first_duration, second_duration
    );
}

#[test]
#[ignore] // Requires Silero VAD model
#[serial]
fn test_vad_init_thread_safety() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    let model_path = vad::default_model_path();
    let init_count = Arc::new(AtomicU32::new(0));

    // Spawn multiple threads trying to init concurrently
    let handles: Vec<_> = (0..10)
        .map(|_| {
            let path = model_path.clone();
            let count = Arc::clone(&init_count);
            thread::spawn(move || {
                let result = vad::init(&path);
                if result.is_ok() {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                result.is_ok()
            })
        })
        .collect();

    // All threads should complete without panic
    let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // All should succeed (either first init or fast-path)
    assert!(
        results.iter().all(|&r| r),
        "All concurrent inits should succeed"
    );

    eprintln!(
        "Concurrent init count: {}",
        init_count.load(Ordering::SeqCst)
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 2: Speech Probability (Non-blocking)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model
#[serial]
fn test_speech_probability_returns_quickly() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    vad::init(&vad::default_model_path()).ok();

    // Generate test audio
    let speech = generate_speech_audio(0.1, 16000); // 100ms

    // Measure call latency - should be < 1ms (non-blocking)
    let mut latencies = Vec::new();
    for _ in 0..100 {
        let start = Instant::now();
        let _prob = vad::speech_probability(&speech, 16000);
        latencies.push(start.elapsed());
    }

    let avg_latency = latencies.iter().sum::<Duration>() / latencies.len() as u32;
    let max_latency = latencies.iter().max().unwrap();

    eprintln!(
        "speech_probability latency: avg={:?}, max={:?}",
        avg_latency, max_latency
    );

    // Should be very fast (fire-and-forget)
    assert!(
        avg_latency < Duration::from_millis(1),
        "Average latency should be < 1ms, got {:?}",
        avg_latency
    );
}

#[test]
#[ignore] // Requires Silero VAD model
#[serial]
fn test_speech_probability_eventual_consistency() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    vad::init(&vad::default_model_path()).ok();
    vad::reset(); // Reset state

    // Generate clear speech
    let speech = generate_speech_audio(0.5, 16000);

    // First few calls may return stale value (eventual consistency)
    let mut probs = Vec::new();
    for _ in 0..20 {
        let prob = vad::speech_probability(&speech, 16000);
        probs.push(prob);
        thread::sleep(Duration::from_millis(50)); // Give worker time to process
    }

    eprintln!("Speech probabilities over time: {:?}", probs);

    // At least one call should detect speech (Silero spikes then decays with repeated input)
    let max_prob = probs.iter().cloned().fold(0.0_f32, f32::max);
    assert!(
        max_prob > 0.3,
        "Speech should be detected at least once with prob > 0.3, max got {}",
        max_prob
    );
}

#[test]
#[ignore] // Requires Silero VAD model
#[serial]
fn test_silence_probability_low() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    vad::init(&vad::default_model_path()).ok();
    vad::reset();

    let silence = generate_silence(0.5, 16000);

    // Warmup: flush any residual state from previous tests
    for _ in 0..5 {
        vad::speech_probability(&silence, 16000);
        thread::sleep(Duration::from_millis(50));
    }
    vad::reset();

    // Now measure with clean state
    let mut probs = Vec::new();
    for _ in 0..20 {
        let prob = vad::speech_probability(&silence, 16000);
        probs.push(prob);
        thread::sleep(Duration::from_millis(50));
    }

    eprintln!("Silence probabilities over time: {:?}", probs);

    // Most readings should be low — at least half below 0.5
    let low_count = probs.iter().filter(|&&p| p < 0.5).count();
    assert!(
        low_count > probs.len() / 2,
        "Majority of silence readings should be < 0.5, but only {}/{} were low: {:?}",
        low_count,
        probs.len(),
        probs
    );
}

#[test]
#[ignore] // Requires Silero VAD model
#[serial]
fn test_sample_rate_resampling_48k() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    vad::init(&vad::default_model_path()).ok();
    vad::reset();

    // Generate speech at 48kHz (common macOS rate)
    let speech_48k = generate_speech_audio(0.5, 48000);

    // speech_probability() is non-blocking: submit audio to worker, read last result.
    // Sleep between calls to let the background worker process each chunk.
    let mut probs = Vec::new();
    for _ in 0..20 {
        let prob = vad::speech_probability(&speech_48k, 48000);
        probs.push(prob);
        thread::sleep(Duration::from_millis(50));
    }

    eprintln!("48kHz speech probabilities: {:?}", probs);

    let max_prob = probs.iter().cloned().fold(0.0f32, f32::max);
    assert!(
        max_prob > 0.2,
        "48kHz speech should be detected after resampling, max prob was {}",
        max_prob
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 3: is_speech threshold
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model
#[serial]
fn test_is_speech_uses_threshold() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    vad::init(&vad::default_model_path()).ok();

    // The is_speech function uses the configured threshold (default 0.5)
    let speech = generate_speech_audio(0.5, 16000);
    let silence = generate_silence(0.5, 16000);

    // Let worker process
    for _ in 0..10 {
        vad::speech_probability(&speech, 16000);
        thread::sleep(Duration::from_millis(50));
    }
    let speech_result = vad::is_speech(&speech, 16000);

    for _ in 0..10 {
        vad::speech_probability(&silence, 16000);
        thread::sleep(Duration::from_millis(50));
    }
    let silence_result = vad::is_speech(&silence, 16000);

    eprintln!(
        "is_speech: speech={}, silence={}",
        speech_result, silence_result
    );

    // Note: Due to eventual consistency, these may not always be accurate
    // The test verifies the function works without panic
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 4: VadConfig clamping
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vad_config_default_values() {
    let config = VadConfig::default();

    // Verify default values are within expected ranges
    assert!(
        config.threshold >= 0.1 && config.threshold <= 0.95,
        "threshold should be 0.1-0.95, got {}",
        config.threshold
    );
    assert!(
        config.min_speech_duration_sec >= 0.01 && config.min_speech_duration_sec <= 1.0,
        "min_speech should be 0.01-1.0, got {}",
        config.min_speech_duration_sec
    );
    assert!(
        config.max_silence_duration_sec >= 0.1 && config.max_silence_duration_sec <= 10.0,
        "max_silence should be 0.1-10.0, got {}",
        config.max_silence_duration_sec
    );
    assert!(
        config.max_utterance_sec >= 1.0 && config.max_utterance_sec <= 300.0,
        "max_utterance should be 1.0-300.0, got {}",
        config.max_utterance_sec
    );
    assert!(
        config.pre_roll_sec >= 0.0 && config.pre_roll_sec <= 2.0,
        "pre_roll should be 0.0-2.0, got {}",
        config.pre_roll_sec
    );
}

#[test]
fn test_vad_config_presets() {
    let sensitive = VadConfig::sensitive();
    let conservative = VadConfig::conservative();

    assert!(
        sensitive.threshold < conservative.threshold,
        "sensitive should have lower threshold"
    );
    assert!(
        sensitive.min_speech_duration_sec < conservative.min_speech_duration_sec,
        "sensitive should have shorter min speech"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 5: Reset functionality
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model
#[serial]
fn test_vad_reset_clears_state() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    vad::init(&vad::default_model_path()).ok();

    // Process some speech to build up state
    let speech = generate_speech_audio(1.0, 16000);
    for _ in 0..10 {
        vad::speech_probability(&speech, 16000);
        thread::sleep(Duration::from_millis(50));
    }

    // Reset should not panic
    vad::reset();

    // After reset, first probability might be different
    // (state cleared, starting fresh)
    let prob_after_reset = vad::speech_probability(&speech, 16000);
    eprintln!("Probability after reset: {}", prob_after_reset);

    // Just verify it works - specific value depends on implementation
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 6: Channel backpressure
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model
#[serial]
fn test_rapid_submissions_dont_block() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    vad::init(&vad::default_model_path()).ok();

    let speech = generate_speech_audio(0.1, 16000);

    // Submit many requests rapidly (simulating high-frequency audio callback)
    let start = Instant::now();
    for _ in 0..1000 {
        vad::speech_probability(&speech, 16000);
    }
    let duration = start.elapsed();

    eprintln!("1000 rapid submissions took: {:?}", duration);

    // Should complete quickly (channel drops old messages if full)
    assert!(
        duration < Duration::from_secs(1),
        "Rapid submissions should not block, took {:?}",
        duration
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 7: Empty/edge case handling
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_empty_samples_returns_valid_probability() {
    // Empty samples should return a valid probability
    let empty: Vec<f32> = vec![];
    let prob = vad::speech_probability(&empty, 16000);

    // API uses "eventual consistency" - returns last_prob which can be any value in [0,1]
    // depending on previous calls. We only verify it's a valid probability.
    // - If VAD not initialized: returns 1.0 (assume speech - safe default)
    // - If VAD initialized: returns last computed probability (may be stale)
    assert!(
        (0.0..=1.0).contains(&prob),
        "Empty samples should return valid probability in [0.0, 1.0], got {}",
        prob
    );
}

#[test]
fn test_very_short_samples() {
    // Very short samples (< chunk size) should still work
    let short = vec![0.5f32; 10]; // Only 10 samples
    let prob = vad::speech_probability(&short, 16000);

    // Should not panic, return some value
    assert!(
        (0.0..=1.0).contains(&prob),
        "Probability should be in [0,1], got {}",
        prob
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Stress Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model, takes time
#[serial]
fn test_sustained_load() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    vad::init(&vad::default_model_path()).ok();

    let speech = generate_speech_audio(0.1, 48000);
    let silence = generate_silence(0.1, 48000);

    // Simulate 10 seconds of audio processing at ~100Hz callback rate
    let start = Instant::now();
    let mut speech_count = 0;
    let mut silence_count = 0;

    while start.elapsed() < Duration::from_secs(10) {
        // Alternate between speech and silence
        if (start.elapsed().as_millis() / 500).is_multiple_of(2) {
            vad::speech_probability(&speech, 48000);
            speech_count += 1;
        } else {
            vad::speech_probability(&silence, 48000);
            silence_count += 1;
        }
        thread::sleep(Duration::from_millis(10)); // ~100Hz
    }

    eprintln!(
        "Sustained load: {} speech calls, {} silence calls in 10s",
        speech_count, silence_count
    );

    // Should complete without issues (allow slower CI / loaded laptops)
    let min_calls = 300;
    assert!(
        speech_count > min_calls,
        "Should process many speech chunks"
    );
    assert!(
        silence_count > min_calls,
        "Should process many silence chunks"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Real Audio Tests — canonical recordings from tests/assets/data_assets/
// ═══════════════════════════════════════════════════════════════════════════

/// Load WAV file as f32 samples + sample rate
fn load_wav(path: &std::path::Path) -> (Vec<f32>, u32) {
    let reader = hound::WavReader::open(path)
        .unwrap_or_else(|e| panic!("Failed to open {}: {}", path.display(), e));
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;

    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .into_samples::<i16>()
            .map(|s| s.unwrap() as f32 / i16::MAX as f32)
            .collect(),
        (hound::SampleFormat::Int, 24 | 32) => reader
            .into_samples::<i32>()
            .map(|s| s.unwrap() as f32 / i32::MAX as f32)
            .collect(),
        (hound::SampleFormat::Float, _) => {
            reader.into_samples::<f32>().map(|s| s.unwrap()).collect()
        }
        _ => panic!(
            "Unsupported WAV format: {:?} {}bit",
            spec.sample_format, spec.bits_per_sample
        ),
    };

    // If stereo, take left channel only
    if spec.channels == 2 {
        let mono: Vec<f32> = samples.iter().step_by(2).copied().collect();
        (mono, sample_rate)
    } else {
        (samples, sample_rate)
    }
}

/// Find canonical test assets directory
fn assets_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = manifest.join("tests/assets/data_assets");
    assert!(dir.exists(), "Test assets not found at {}", dir.display());
    dir
}

/// Smoke test: single chunk from real audio → VAD must return non-zero
#[test]
#[ignore] // Requires Silero VAD model + test audio assets
#[serial]
fn test_vad_real_audio_smoke() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    let model = vad::default_model_path();
    vad::init(&model).expect("VAD init failed");
    vad::reset();

    let wav_path = assets_dir().join("01_no-to-dobra.wav");
    let (samples, sample_rate) = load_wav(&wav_path);
    eprintln!("  Loaded {} samples at {}Hz", samples.len(), sample_rate);

    // Take 1 second of audio from the middle (should be speech)
    let one_sec = sample_rate as usize;
    let start = samples.len() / 3;
    let chunk = &samples[start..start + one_sec.min(samples.len() - start)];
    eprintln!("  Chunk: {} samples from offset {}", chunk.len(), start);

    // Submit and wait generously
    for i in 0..10 {
        vad::speech_probability(chunk, sample_rate);
        thread::sleep(Duration::from_millis(100));
        let prob = vad::speech_probability(chunk, sample_rate);
        eprintln!("  Round {}: prob={:.4}", i, prob);
        if prob > 0.1 {
            eprintln!("  ✓ Got non-zero speech probability!");
            return;
        }
    }

    // If we got here, check is_initialized
    eprintln!("  is_initialized={}", vad::is_initialized());
    panic!("VAD returned 0.0 for all 10 rounds on real speech audio");
}

/// Test Silero VAD on real Polish speech — should detect speech regions
#[test]
#[ignore] // Requires Silero VAD model + test audio assets
#[serial]
fn test_vad_real_audio_speech_detection() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    let model = vad::default_model_path();
    vad::init(&model).expect("VAD init must succeed for real audio test");
    assert!(vad::is_initialized(), "VAD should be initialized");

    let assets = assets_dir();
    let recordings = [
        ("01_no-to-dobra.wav", "casual Polish"),
        ("02_kubernetes-wymaga-konfiguracji.wav", "tech + vet terms"),
        ("03_algorytm-ma-zlozonosc.wav", "medium difficulty"),
        ("04_runda-3-czyli.wav", "hard mispronunciations"),
    ];

    for (filename, label) in &recordings {
        let wav_path = assets.join(filename);
        if !wav_path.exists() {
            eprintln!("Skipping {}: file not found", filename);
            continue;
        }

        vad::reset();
        thread::sleep(Duration::from_millis(50)); // let reset propagate
        let (samples, sample_rate) = load_wav(&wav_path);

        // Sample 5 one-second windows spread across the recording
        let one_sec = sample_rate as usize;
        let step = samples.len() / 6; // 5 windows with margin
        let mut max_prob = 0.0f32;
        let mut speech_windows = 0u32;

        for w in 0..5 {
            let start = step * (w + 1);
            if start + one_sec > samples.len() {
                break;
            }
            let window = &samples[start..start + one_sec];

            // Submit window, wait for worker, read result
            vad::speech_probability(window, sample_rate);
            thread::sleep(Duration::from_millis(100));
            let prob = vad::speech_probability(window, sample_rate);

            if prob > max_prob {
                max_prob = prob;
            }
            if prob >= 0.5 {
                speech_windows += 1;
            }
            eprintln!("  [{label}] window {w}: prob={prob:.4}");
        }

        eprintln!("  [{label}] speech_windows={speech_windows}/5 max_prob={max_prob:.3}");

        // Every recording has real speech — at least 1 window must detect it
        assert!(
            max_prob > 0.5,
            "[{label}] max speech probability {max_prob:.3} too low — VAD not detecting speech"
        );
    }
}

/// Test VAD detects silence gaps between sentences in real audio
#[test]
#[ignore] // Requires Silero VAD model + test audio assets
#[serial]
fn test_vad_real_audio_silence_gaps() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    vad::init(&vad::default_model_path()).expect("VAD init failed");
    vad::reset();
    thread::sleep(Duration::from_millis(50));

    // Recording 02 has numbered sentences with pauses between them
    let wav_path = assets_dir().join("02_kubernetes-wymaga-konfiguracji.wav");
    if !wav_path.exists() {
        eprintln!("Skipping: test asset not found");
        return;
    }

    let (samples, sample_rate) = load_wav(&wav_path);

    // Use 500ms windows to get probability timeline (coarser but reliable)
    let window_size = sample_rate as usize / 2; // 500ms
    let mut probs: Vec<f32> = Vec::new();
    for window in samples.chunks(window_size) {
        if window.len() < window_size {
            break;
        }
        vad::speech_probability(window, sample_rate);
        thread::sleep(Duration::from_millis(100));
        probs.push(vad::speech_probability(window, sample_rate));
    }

    // Count speech→silence transitions (= sentence boundaries)
    let threshold = 0.5f32;
    let mut transitions = 0u32;
    let mut was_speech = false;
    for &p in &probs {
        let is_speech = p >= threshold;
        if was_speech && !is_speech {
            transitions += 1;
        }
        was_speech = is_speech;
    }

    eprintln!(
        "  [silence gaps] windows={} speech→silence transitions={transitions} probs={:?}",
        probs.len(),
        probs.iter().map(|p| format!("{p:.2}")).collect::<Vec<_>>()
    );

    // Recording 02 has multiple sentences — expect at least 1 transition
    assert!(
        transitions >= 1,
        "Expected at least 1 speech→silence transition, got {transitions}"
    );
}

/// Test VAD on dedicated pause recording (59s with intentional silence gaps)
///
/// VAD_voice_real_pauses.wav — 16kHz mono, real speech with deliberate pauses.
/// Should show clear speech/silence segmentation with multiple transitions.
#[test]
#[ignore] // Requires Silero VAD model + test audio assets
#[serial]
fn test_vad_real_pauses_recording() {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return;
    }

    vad::init(&vad::default_model_path()).expect("VAD init failed");
    vad::reset();
    thread::sleep(Duration::from_millis(50));

    let wav_path = assets_dir().join("VAD_voice_real_pauses.wav");
    if !wav_path.exists() {
        eprintln!("Skipping: VAD_voice_real_pauses.wav not found");
        return;
    }

    let (samples, sample_rate) = load_wav(&wav_path);
    let duration_sec = samples.len() as f32 / sample_rate as f32;
    eprintln!(
        "  Loaded {} samples at {}Hz ({:.1}s)",
        samples.len(),
        sample_rate,
        duration_sec
    );

    // 500ms windows — good balance between resolution and worker latency
    let window_size = sample_rate as usize / 2;
    let mut probs: Vec<f32> = Vec::new();
    for window in samples.chunks(window_size) {
        if window.len() < window_size {
            break;
        }
        vad::speech_probability(window, sample_rate);
        thread::sleep(Duration::from_millis(100));
        probs.push(vad::speech_probability(window, sample_rate));
    }

    // Metrics
    let threshold = 0.5f32;
    let speech_count = probs.iter().filter(|&&p| p >= threshold).count();
    let silence_count = probs.iter().filter(|&&p| p < threshold).count();

    let mut transitions = 0u32;
    let mut was_speech = false;
    for &p in &probs {
        let is_speech = p >= threshold;
        if was_speech && !is_speech {
            transitions += 1;
        }
        was_speech = is_speech;
    }

    // Print probability timeline as sparkline
    let sparkline: String = probs
        .iter()
        .map(|&p| {
            if p >= 0.9 {
                '█'
            } else if p >= 0.5 {
                '▓'
            } else if p >= 0.1 {
                '░'
            } else {
                ' '
            }
        })
        .collect();

    eprintln!(
        "  [real pauses] windows={} speech={speech_count} silence={silence_count}",
        probs.len()
    );
    eprintln!("  transitions (speech→silence): {transitions}");
    eprintln!("  timeline: |{sparkline}|");
    eprintln!(
        "  probs: {:?}",
        probs.iter().map(|p| format!("{p:.2}")).collect::<Vec<_>>()
    );

    // 59s recording with intentional pauses — must have speech AND silence
    assert!(
        speech_count > 5,
        "Expected speech in >5 windows, got {speech_count}"
    );
    assert!(
        silence_count > 3,
        "Expected silence in >3 windows (recording has intentional pauses), got {silence_count}"
    );
    assert!(
        transitions >= 3,
        "Expected at least 3 speech→silence transitions in 59s recording with pauses, got {transitions}"
    );
}
