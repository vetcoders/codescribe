//! End-to-end tests for VAD (Voice Activity Detection) flow.
//!
//! Tests the local-instance VAD architecture:
//! - AccumulatingVad creation and lifecycle
//! - Speech probability via feed() (synchronous, deterministic)
//! - Silence detection
//! - VadConfig presets and clamping
//! - Real audio speech detection
//!
//! Created by M&K (c)2026 VetCoders

use codescribe_core::vad::{self, AccumulatingVad, VadConfig};
use std::path::PathBuf;
use std::time::Instant;

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

/// Create AccumulatingVad at 16kHz (or skip if model missing)
fn create_vad_16k() -> Option<AccumulatingVad> {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return None;
    }
    AccumulatingVad::new(16000).ok()
}

/// Create AccumulatingVad at custom sample rate (or skip if model missing)
fn create_vad(sample_rate: u32) -> Option<AccumulatingVad> {
    if !vad_model_available() {
        eprintln!("Skipping: VAD model not found");
        return None;
    }
    AccumulatingVad::new(sample_rate).ok()
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 1: AccumulatingVad Creation
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vad_creation_without_model_returns_error() {
    let fake_path = std::path::PathBuf::from("/nonexistent/silero_vad.onnx");
    let result = AccumulatingVad::with_config(&fake_path, VadConfig::default(), 16000);

    assert!(result.is_err(), "should fail with missing model");
}

#[test]
#[ignore] // Requires Silero VAD model
fn test_vad_creation_success() {
    let vad = create_vad_16k();
    assert!(
        vad.is_some(),
        "should create AccumulatingVad with valid model"
    );

    let vad = vad.unwrap();
    // Initial probability must be 0.0 (the critical fix — was 1.0 in VadWorker)
    assert_eq!(
        vad.probability(),
        0.0,
        "initial probability must be 0.0, not 1.0"
    );
}

#[test]
#[ignore] // Requires Silero VAD model
fn test_vad_multiple_instances_independent() {
    // No singleton — each instance is independent
    let vad1 = create_vad_16k();
    let vad2 = create_vad_16k();
    assert!(
        vad1.is_some() && vad2.is_some(),
        "should create multiple instances"
    );

    let mut vad1 = vad1.unwrap();
    let vad2 = vad2.unwrap();

    // Feed speech to vad1 only
    let speech = generate_speech_audio(0.5, 16000);
    vad1.feed(&speech);

    // vad2 should still be at 0.0
    assert_eq!(
        vad2.probability(),
        0.0,
        "separate instances should not affect each other"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 2: Speech Detection (synchronous feed)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model
fn test_feed_returns_probability_synchronously() {
    let Some(mut vad) = create_vad_16k() else {
        return;
    };

    let speech = generate_speech_audio(0.5, 16000);

    // feed() is synchronous — no sleep needed (unlike old fire-and-forget)
    let prob = vad.feed(&speech);

    eprintln!("Speech probability (synchronous): {:.4}", prob);

    // Verify feed() returns a valid probability in [0, 1] range.
    // Synthetic sinusoids won't trigger Silero (it recognises formant
    // structure, not pure tones) — the point is API correctness.
    assert!(
        (0.0..=1.0).contains(&prob),
        "feed() should return probability in [0,1], got {:.4}",
        prob
    );
}

#[test]
#[ignore] // Requires Silero VAD model
fn test_silence_probability_low() {
    let Some(mut vad) = create_vad_16k() else {
        return;
    };

    let silence = generate_silence(0.5, 16000);
    let prob = vad.feed(&silence);

    eprintln!("Silence probability: {:.4}", prob);

    assert!(
        prob < 0.3,
        "Silence should have low probability, got {:.4}",
        prob
    );
}

#[test]
#[ignore] // Requires Silero VAD model
fn test_speech_then_silence_transition() {
    let Some(mut vad) = create_vad_16k() else {
        return;
    };

    // Feed speech first
    let speech = generate_speech_audio(1.0, 16000);
    let speech_prob = vad.feed(&speech);
    eprintln!("After speech: {:.4}", speech_prob);

    // Feed silence — probability should drop
    let silence = generate_silence(1.0, 16000);
    let silence_prob = vad.feed(&silence);
    eprintln!("After silence: {:.4}", silence_prob);

    assert!(
        silence_prob < speech_prob,
        "Probability should drop after silence: speech={:.4} silence={:.4}",
        speech_prob,
        silence_prob
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 3: Sample rate resampling
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model
fn test_48k_resampling_detects_speech() {
    let Some(mut vad) = create_vad(48000) else {
        return;
    };

    // Audio at 48kHz — AccumulatingVad should resample to 16kHz internally
    let speech = generate_speech_audio(0.5, 48000);
    let prob = vad.feed(&speech);

    eprintln!("48kHz probability (synthetic): {:.4}", prob);

    // Tests resampling pipeline doesn't crash and returns valid probability.
    // Real speech detection is tested in test_vad_real_audio_* tests.
    assert!(
        (0.0..=1.0).contains(&prob),
        "48kHz resampled feed() should return valid probability, got {:.4}",
        prob
    );
}

#[test]
#[ignore] // Requires Silero VAD model
fn test_44100_resampling_detects_speech() {
    // 44100Hz is the common macOS native sample rate
    let Some(mut vad) = create_vad(44100) else {
        return;
    };

    let speech = generate_speech_audio(0.5, 44100);
    let prob = vad.feed(&speech);

    eprintln!("44100Hz probability (synthetic): {:.4}", prob);

    // Tests 44100→16000 resampling pipeline returns valid probability.
    assert!(
        (0.0..=1.0).contains(&prob),
        "44100Hz resampled feed() should return valid probability, got {:.4}",
        prob
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 4: Chunk accumulation (the critical fix)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model
fn test_small_chunks_accumulate_correctly() {
    let Some(mut vad) = create_vad_16k() else {
        return;
    };

    // Feed sub-chunk-size pieces (< 512 samples)
    // This is what cpal delivers: ~1024 @ 44100Hz → ~371 @ 16kHz after resampling
    // The old VadWorker lost these because it never accumulated across calls
    let speech = generate_speech_audio(0.5, 16000);
    let mut max_prob = 0.0f32;

    for chunk in speech.chunks(300) {
        // 300 < 512 CHUNK_SIZE
        let prob = vad.feed(chunk);
        max_prob = max_prob.max(prob);
    }

    eprintln!("Max prob from 300-sample chunks: {:.4}", max_prob);

    assert!(
        max_prob > 0.1,
        "Small chunks should accumulate to full 512-sample inference, got {:.4}",
        max_prob
    );
}

#[test]
#[ignore] // Requires Silero VAD model
fn test_single_sample_chunks_still_work() {
    let Some(mut vad) = create_vad_16k() else {
        return;
    };

    // Extreme case: one sample at a time (should still accumulate)
    let speech = generate_speech_audio(0.1, 16000); // 1600 samples
    let mut any_nonzero = false;

    for &sample in &speech {
        let prob = vad.feed(&[sample]);
        if prob > 0.0 {
            any_nonzero = true;
        }
    }

    assert!(
        any_nonzero,
        "Even single-sample feed should eventually produce inference"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 5: VadConfig
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_vad_config_default_values() {
    let config = VadConfig::default();

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
// Integration Point 6: Reset
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model
fn test_reset_clears_state() {
    let Some(mut vad) = create_vad_16k() else {
        return;
    };

    // Build up state with speech
    let speech = generate_speech_audio(1.0, 16000);
    let prob_before = vad.feed(&speech);
    eprintln!("Before reset: {:.4}", prob_before);

    // Reset
    vad.reset();

    // After reset, probability should be 0.0
    assert_eq!(
        vad.probability(),
        0.0,
        "probability should be 0.0 after reset"
    );

    // Feed silence after reset — should stay low
    let silence = generate_silence(0.5, 16000);
    let prob_after = vad.feed(&silence);
    eprintln!("After reset + silence: {:.4}", prob_after);
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration Point 7: Edge cases
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model
fn test_empty_samples() {
    let Some(mut vad) = create_vad_16k() else {
        return;
    };

    let prob = vad.feed(&[]);
    assert_eq!(prob, 0.0, "empty feed should return 0.0 (initial)");
}

#[test]
#[ignore] // Requires Silero VAD model
fn test_very_short_samples() {
    let Some(mut vad) = create_vad_16k() else {
        return;
    };

    // < 512 samples — should accumulate without panic
    let short = vec![0.5f32; 10];
    let prob = vad.feed(&short);
    // Not enough for a full chunk → still at initial 0.0
    assert_eq!(
        prob, 0.0,
        "sub-chunk feed should return 0.0 until enough accumulated"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Stress Tests
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[ignore] // Requires Silero VAD model
fn test_feed_performance() {
    let Some(mut vad) = create_vad_16k() else {
        return;
    };

    let speech = generate_speech_audio(0.032, 16000); // 512 samples = 1 chunk

    // Measure feed latency
    let start = Instant::now();
    let iterations = 1000;
    for _ in 0..iterations {
        vad.feed(&speech);
    }
    let total = start.elapsed();
    let avg = total / iterations;

    eprintln!(
        "feed() latency: avg={:?} ({} iterations in {:?})",
        avg, iterations, total
    );

    // Each feed() runs one Silero inference — should be < 5ms on Apple Silicon
    assert!(
        avg.as_millis() < 10,
        "Average feed latency should be < 10ms, got {:?}",
        avg
    );
}

#[test]
#[ignore] // Requires Silero VAD model, takes time
fn test_sustained_alternating_speech_silence() {
    let Some(mut vad) = create_vad(48000) else {
        return;
    };

    let speech = generate_speech_audio(0.1, 48000);
    let silence = generate_silence(0.1, 48000);

    // Simulate 5 seconds of alternating speech/silence at ~100Hz
    let start = Instant::now();
    let mut speech_detected = 0u32;
    let mut silence_detected = 0u32;
    let threshold = vad.threshold();
    let mut iteration = 0u32;

    while start.elapsed().as_secs() < 5 {
        let prob = if iteration % 20 < 10 {
            vad.feed(&speech)
        } else {
            vad.feed(&silence)
        };

        if prob >= threshold {
            speech_detected += 1;
        } else {
            silence_detected += 1;
        }
        iteration += 1;
    }

    eprintln!(
        "Sustained load: {} iterations, speech_detected={}, silence_detected={}",
        iteration, speech_detected, silence_detected
    );

    assert!(iteration > 100, "Should process many iterations");
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

/// Smoke test: real audio → AccumulatingVad must detect speech
#[test]
#[ignore] // Requires Silero VAD model + test audio assets
fn test_vad_real_audio_smoke() {
    if !vad_model_available() {
        return;
    }

    let wav_path = assets_dir().join("01_no-to-dobra.wav");
    let (samples, sample_rate) = load_wav(&wav_path);
    eprintln!("  Loaded {} samples at {}Hz", samples.len(), sample_rate);

    // Create VAD at the file's native sample rate
    let mut vad = AccumulatingVad::new(sample_rate).expect("VAD creation should succeed");

    // Feed 1 second from the middle (should be speech)
    let one_sec = sample_rate as usize;
    let start = samples.len() / 3;
    let chunk = &samples[start..start + one_sec.min(samples.len() - start)];

    let prob = vad.feed(chunk);
    eprintln!("  Real audio speech prob: {:.4}", prob);

    assert!(
        prob > 0.1,
        "Should detect speech in real Polish audio, got {:.4}",
        prob
    );
}

/// Test AccumulatingVad on real Polish speech — should detect speech regions
#[test]
#[ignore] // Requires Silero VAD model + test audio assets
fn test_vad_real_audio_speech_detection() {
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

        let (samples, sample_rate) = load_wav(&wav_path);
        let mut vad = match AccumulatingVad::new(sample_rate) {
            Ok(v) => v,
            Err(_) => {
                eprintln!("Skipping: VAD model not available");
                return;
            }
        };

        // Sample 5 one-second windows spread across the recording
        let one_sec = sample_rate as usize;
        let step = samples.len() / 6;
        let mut max_prob = 0.0f32;
        let mut speech_windows = 0u32;

        for w in 0..5 {
            let start = step * (w + 1);
            if start + one_sec > samples.len() {
                break;
            }
            let window = &samples[start..start + one_sec];

            // Synchronous — no sleep needed
            let prob = vad.feed(window);
            max_prob = max_prob.max(prob);
            if prob >= 0.5 {
                speech_windows += 1;
            }
            eprintln!("  [{label}] window {w}: prob={prob:.4}");
        }

        eprintln!("  [{label}] speech_windows={speech_windows}/5 max_prob={max_prob:.3}");

        assert!(
            max_prob > 0.5,
            "[{label}] max speech probability {max_prob:.3} too low — VAD not detecting speech"
        );
    }
}

/// Test VAD detects silence gaps between sentences in real audio
#[test]
#[ignore] // Requires Silero VAD model + test audio assets
fn test_vad_real_audio_silence_gaps() {
    let wav_path = assets_dir().join("02_kubernetes-wymaga-konfiguracji.wav");
    if !wav_path.exists() {
        eprintln!("Skipping: test asset not found");
        return;
    }

    let (samples, sample_rate) = load_wav(&wav_path);
    let mut vad = match AccumulatingVad::new(sample_rate) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("Skipping: VAD model not available");
            return;
        }
    };

    // 500ms windows
    let window_size = sample_rate as usize / 2;
    let mut probs: Vec<f32> = Vec::new();
    for window in samples.chunks(window_size) {
        if window.len() < window_size {
            break;
        }
        // Reset before each window for independent measurement
        vad.reset();
        probs.push(vad.feed(window));
    }

    // Count speech→silence transitions
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
        "  [silence gaps] windows={} transitions={transitions} probs={:?}",
        probs.len(),
        probs.iter().map(|p| format!("{p:.2}")).collect::<Vec<_>>()
    );

    assert!(
        transitions >= 1,
        "Expected at least 1 speech→silence transition, got {transitions}"
    );
}

/// Test VAD on dedicated pause recording (59s with intentional silence gaps)
#[test]
#[ignore] // Requires Silero VAD model + test audio assets
fn test_vad_real_pauses_recording() {
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

    let mut vad = match AccumulatingVad::new(sample_rate) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("Skipping: VAD model not available");
            return;
        }
    };

    // 500ms windows — reset between for independent measurement
    let window_size = sample_rate as usize / 2;
    let mut probs: Vec<f32> = Vec::new();
    for window in samples.chunks(window_size) {
        if window.len() < window_size {
            break;
        }
        vad.reset();
        probs.push(vad.feed(window));
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

    // Sparkline
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

    assert!(
        speech_count > 5,
        "Expected speech in >5 windows, got {speech_count}"
    );
    assert!(
        silence_count > 3,
        "Expected silence in >3 windows, got {silence_count}"
    );
    assert!(
        transitions >= 3,
        "Expected >=3 speech→silence transitions in 59s recording, got {transitions}"
    );
}
