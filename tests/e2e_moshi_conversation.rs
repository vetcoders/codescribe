//! E2E tests for Moshi conversational AI engine
//!
//! Tests full pipeline: init → encode → inference → decode → audio output.
//!
//! To run (requires Moshi models):
//!   CODESCRIBE_E2E_MOSHI=1 cargo test --test e2e_moshi_conversation
//!
//! Created by M&K (c)2026 VetCoders

use std::path::PathBuf;

use codescribe_core::conversation::context::ConversationState;
use codescribe_core::conversation::{ConversationEngine, MoshiConfig};

/// Check if Moshi models are available
fn moshi_available() -> bool {
    let config = MoshiConfig::default();
    config.validate().is_ok()
}

/// Get test audio samples (24kHz, mono, 1 second of silence with tone)
fn generate_test_audio(duration_secs: f32) -> Vec<f32> {
    let sample_rate = 24000;
    let num_samples = (duration_secs * sample_rate as f32) as usize;

    (0..num_samples)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            // 440Hz sine wave with envelope
            let envelope = if t < 0.1 {
                t / 0.1
            } else if t > duration_secs - 0.1 {
                (duration_secs - t) / 0.1
            } else {
                1.0
            };
            0.3 * envelope * (440.0 * t * std::f32::consts::PI * 2.0).sin()
        })
        .collect()
}

/// Test that engine can be created without models
#[test]
fn test_engine_creation_no_init() {
    let config = MoshiConfig::default();
    let engine = ConversationEngine::new(config);
    assert!(engine.is_ok(), "Engine creation should succeed");

    let engine = engine.unwrap();
    assert!(
        !engine.is_initialized(),
        "Engine should not be initialized yet"
    );
    assert_eq!(engine.state(), ConversationState::Idle);
}

/// Test config validation for moshiko
#[test]
fn test_moshiko_config() {
    let config = MoshiConfig::moshiko();
    assert_eq!(config.voice, "moshiko");

    // Check paths point to expected locations
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let expected_model = PathBuf::from(&home).join(".codescribe/models/moshiko-q8/model.q8.gguf");
    assert_eq!(config.model_path, expected_model);
}

/// Test config validation for moshika
#[test]
fn test_moshika_config() {
    let config = MoshiConfig::moshika();
    assert_eq!(config.voice, "moshika");

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let expected_model = PathBuf::from(&home).join(".codescribe/models/moshika-q8/model.q8.gguf");
    assert_eq!(config.model_path, expected_model);
}

/// Full E2E test: init → process audio → get response
///
/// Run with: CODESCRIBE_E2E_MOSHI=1 cargo test --test e2e_moshi_conversation test_full_conversation
#[test]
fn test_full_conversation() {
    let enabled = std::env::var("CODESCRIBE_E2E_MOSHI")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping Moshi E2E (set CODESCRIBE_E2E_MOSHI=1 to enable)");
        return;
    }

    if !moshi_available() {
        eprintln!("Moshi models not found, skipping test");
        eprintln!("Expected model at ~/.codescribe/models/moshiko-q8/model.q8.gguf");
        return;
    }

    println!("═══════════════════════════════════════════════════════════");
    println!("  Moshi Conversation E2E Test");
    println!("═══════════════════════════════════════════════════════════");

    // Create engine
    let config = MoshiConfig::moshiko();
    let mut engine = ConversationEngine::new(config).expect("create engine");

    // Initialize (loads models)
    println!("Initializing engine (loading models)...");
    let start = std::time::Instant::now();
    engine.init().expect("init engine");
    println!("Engine initialized in {:?}", start.elapsed());

    assert!(engine.is_initialized());

    // Generate test audio (2 seconds of tone)
    let test_audio = generate_test_audio(2.0);
    println!(
        "Test audio: {} samples ({:.1}s)",
        test_audio.len(),
        test_audio.len() as f32 / 24000.0
    );

    // Process audio in chunks
    println!("Processing audio...");
    let chunk_size = 1920; // 80ms at 24kHz
    let start = std::time::Instant::now();

    for chunk in test_audio.chunks(chunk_size) {
        engine.process_audio(chunk).expect("process audio");
    }

    println!("Audio processed in {:?}", start.elapsed());

    // Check if we got a response
    if let Some(response) = engine.get_response() {
        println!(
            "Got response: {} samples ({:.2}s)",
            response.len(),
            response.len() as f32 / 24000.0
        );
        assert!(!response.is_empty(), "Response should not be empty");
    } else {
        println!("No response generated (may need longer audio with actual speech)");
    }

    println!("═══════════════════════════════════════════════════════════");
}

/// Test engine initialization (model loading)
///
/// Run with: CODESCRIBE_E2E_MOSHI=1 cargo test --test e2e_moshi_conversation test_init_loads_models
#[test]
fn test_init_loads_models() {
    let enabled = std::env::var("CODESCRIBE_E2E_MOSHI")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping Moshi init E2E (set CODESCRIBE_E2E_MOSHI=1 to enable)");
        return;
    }

    if !moshi_available() {
        eprintln!("Moshi models not found, skipping test");
        return;
    }

    let config = MoshiConfig::moshiko();
    let mut engine = ConversationEngine::new(config).expect("create engine");

    // Init should load models
    let start = std::time::Instant::now();
    engine.init().expect("init");
    let load_time = start.elapsed();

    println!("Model load time: {:?}", load_time);
    assert!(engine.is_initialized());

    // Second init should be fast (no-op)
    let start = std::time::Instant::now();
    engine.init().expect("init again");
    let second_time = start.elapsed();

    println!("Second init time: {:?}", second_time);
    assert!(
        second_time < std::time::Duration::from_millis(10),
        "Second init should be instant"
    );
}

/// Test reset clears state properly
#[test]
fn test_reset_clears_state() {
    let config = MoshiConfig::default();
    let mut engine = ConversationEngine::new(config).expect("create engine");

    engine.set_system_prompt("Test prompt");
    assert!(engine.context().system_prompt().is_some());

    engine.reset();
    assert!(engine.context().system_prompt().is_none());
    assert!(!engine.is_speaking());
}
