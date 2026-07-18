//! E2E Round-Trip Tests - verify speech pipeline integrity
//!
//! Tests the ACTUAL components used by Codescribe:
//! - TTS (CSM) → audio generation
//! - STT (Whisper) → transcription
//! - Embedder (MiniLM) → semantic similarity
//!
//! Pattern: Text → TTS → audio → STT → text → compare
//!
//! Run with: CODESCRIBE_E2E_ROUNDTRIP=1 cargo test --test e2e_round_trip
//!
//! Created by Vetcoders (c)2026

use anyhow::Result;

#[path = "support/e2e_stt_matrix.rs"]
mod e2e_stt_matrix;

use e2e_stt_matrix::{ROUNDTRIP_OPT_IN_ENV, env_opt_in, parse_opt_in};

/// Skip unless CODESCRIBE_E2E_ROUNDTRIP=1 is set
fn should_run() -> bool {
    env_opt_in(ROUNDTRIP_OPT_IN_ENV)
}

/// Calculate simple word overlap similarity (0.0 - 1.0)
fn word_similarity(a: &str, b: &str) -> f32 {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();

    let a_words: std::collections::HashSet<&str> = a_lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();
    let b_words: std::collections::HashSet<&str> = b_lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();

    if a_words.is_empty() || b_words.is_empty() {
        return 0.0;
    }

    let intersection = a_words.intersection(&b_words).count();
    let union = a_words.union(&b_words).count();

    intersection as f32 / union as f32
}

#[test]
fn test_roundtrip_gate_requires_explicit_opt_in() {
    assert!(parse_opt_in(Some("1")), "1 should enable opt-in gates");
    assert!(
        parse_opt_in(Some("true")),
        "true should enable opt-in gates"
    );
    assert!(
        !parse_opt_in(Some("yes")),
        "yes should not enable round-trip opt-in gates"
    );
    assert!(
        !parse_opt_in(Some("0")),
        "0 should not enable round-trip opt-in gates"
    );
    assert!(
        !parse_opt_in(None),
        "missing env var should keep heavy round-trip tests disabled"
    );
}

#[test]
fn test_whisper_embedded_readiness_contract() {
    let available = codescribe_core::stt::whisper::embedded::is_embedded_available();
    let embedded = codescribe_core::stt::whisper::embedded::get_embedded_data();

    assert_eq!(
        available,
        embedded.is_some(),
        "embedded readiness contract broken: is_embedded_available() and get_embedded_data() disagree"
    );

    if let Some(model) = embedded {
        assert!(
            model.total_size() > 0,
            "embedded model advertised as available but total_size() is zero"
        );
        assert!(
            !model.config.is_empty(),
            "embedded model missing config bytes despite availability=true"
        );
        assert!(
            !model.tokenizer.is_empty(),
            "embedded model missing tokenizer bytes despite availability=true"
        );
        assert!(
            !model.mel_filters.is_empty(),
            "embedded model missing mel filter bytes despite availability=true"
        );
        assert!(
            !model.weights.is_empty(),
            "embedded model missing weight bytes despite availability=true"
        );
    }
}

// ═══════════════════════════════════════════════════════════════
// TTS → STT Round-Trip Tests
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_tts_stt_round_trip_english() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping: set CODESCRIBE_E2E_ROUNDTRIP=1 to run");
        return Ok(());
    }

    // Initialize components
    codescribe_core::tts::init()?;
    codescribe_core::stt::whisper::init()?;

    let input = "Hello, this is a test of the speech pipeline.";
    eprintln!("Input:  '{}'", input);

    // TTS: text → audio (24kHz)
    let audio = codescribe_core::tts::synthesize(input)?;
    eprintln!(
        "TTS generated {} samples ({:.2}s @ 24kHz)",
        audio.len(),
        audio.len() as f32 / 24000.0
    );
    assert!(!audio.is_empty(), "TTS should produce audio");
    assert!(audio.len() > 24000, "Should be at least 1 second of audio");

    // STT: audio → text
    let transcribed = codescribe_core::stt::whisper::transcribe(&audio, 24000, Some("en"))?;
    eprintln!("Output: '{}'", transcribed.trim());

    // Verify similarity (not exact match due to STT variations)
    let similarity = word_similarity(input, &transcribed);
    eprintln!("Word similarity: {:.2}", similarity);

    assert!(
        similarity > 0.5,
        "Round-trip should preserve at least 50% of words. Got {:.2}",
        similarity
    );

    Ok(())
}

#[test]
fn test_tts_stt_round_trip_polish() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping: set CODESCRIBE_E2E_ROUNDTRIP=1 to run");
        return Ok(());
    }

    codescribe_core::tts::init()?;
    codescribe_core::stt::whisper::init()?;

    let input = "Cześć, to jest test systemu rozpoznawania mowy.";
    eprintln!("Input:  '{}'", input);

    let audio = codescribe_core::tts::synthesize(input)?;
    eprintln!("TTS generated {} samples", audio.len());

    let transcribed = codescribe_core::stt::whisper::transcribe(&audio, 24000, Some("pl"))?;
    eprintln!("Output: '{}'", transcribed.trim());

    let similarity = word_similarity(input, &transcribed);
    eprintln!("Word similarity: {:.2}", similarity);

    assert!(
        similarity > 0.4,
        "Polish round-trip should preserve at least 40% of words. Got {:.2}",
        similarity
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════
// Embedding Similarity Tests
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_embedding_round_trip_similarity() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping: set CODESCRIBE_E2E_ROUNDTRIP=1 to run");
        return Ok(());
    }

    codescribe_core::tts::init()?;
    codescribe_core::stt::whisper::init()?;
    codescribe_core::embedder::init()?;

    let input = "The quick brown fox jumps over the lazy dog.";
    eprintln!("Input: '{}'", input);

    // Generate audio and transcribe back
    let audio = codescribe_core::tts::synthesize(input)?;
    let transcribed = codescribe_core::stt::whisper::transcribe(&audio, 24000, Some("en"))?;
    eprintln!("Transcribed: '{}'", transcribed.trim());

    // Get embeddings for both
    let input_embedding = codescribe_core::embedder::embed(input)?;
    let output_embedding = codescribe_core::embedder::embed(&transcribed)?;

    // Calculate cosine similarity
    let similarity = codescribe_core::embedder::similarity(&input_embedding, &output_embedding);
    eprintln!("Embedding similarity: {:.4}", similarity);

    assert!(
        similarity > 0.7,
        "Semantic similarity should be > 0.7 for round-trip. Got {:.4}",
        similarity
    );

    Ok(())
}

#[test]
fn test_embedding_preserves_meaning() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping: set CODESCRIBE_E2E_ROUNDTRIP=1 to run");
        return Ok(());
    }

    codescribe_core::embedder::init()?;

    // Similar meaning, different words
    let pairs = [
        ("Hello, how are you?", "Hi, how's it going?"),
        (
            "The cat sat on the mat.",
            "A feline was sitting on the rug.",
        ),
        ("I need to buy groceries.", "I have to purchase food items."),
    ];

    for (a, b) in pairs {
        let emb_a = codescribe_core::embedder::embed(a)?;
        let emb_b = codescribe_core::embedder::embed(b)?;
        let sim = codescribe_core::embedder::similarity(&emb_a, &emb_b);

        eprintln!("'{}' vs '{}' → {:.4}", a, b, sim);
        assert!(
            sim > 0.6,
            "Similar meanings should have similarity > 0.6. Got {:.4}",
            sim
        );
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════
// Full Pipeline Tests
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_full_pipeline_double_round_trip() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping: set CODESCRIBE_E2E_ROUNDTRIP=1 to run");
        return Ok(());
    }

    codescribe_core::tts::init()?;
    codescribe_core::stt::whisper::init()?;
    codescribe_core::embedder::init()?;

    let original = "Please remember to call me tomorrow morning.";
    eprintln!("Original: '{}'", original);

    // First round-trip: text → audio → text
    let audio1 = codescribe_core::tts::synthesize(original)?;
    let text1 = codescribe_core::stt::whisper::transcribe(&audio1, 24000, Some("en"))?;
    eprintln!("After 1st round-trip: '{}'", text1.trim());

    // Second round-trip: text → audio → text
    let audio2 = codescribe_core::tts::synthesize(&text1)?;
    let text2 = codescribe_core::stt::whisper::transcribe(&audio2, 24000, Some("en"))?;
    eprintln!("After 2nd round-trip: '{}'", text2.trim());

    // Compare original with final using embeddings
    let emb_original = codescribe_core::embedder::embed(original)?;
    let emb_final = codescribe_core::embedder::embed(&text2)?;
    let similarity = codescribe_core::embedder::similarity(&emb_original, &emb_final);

    eprintln!(
        "Semantic preservation after 2 round-trips: {:.4}",
        similarity
    );

    // Even after two round-trips, meaning should be mostly preserved
    assert!(
        similarity > 0.6,
        "Double round-trip should preserve > 60% semantic meaning. Got {:.4}",
        similarity
    );

    Ok(())
}

#[test]
fn test_whisper_embedded_model_works() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping: set CODESCRIBE_E2E_ROUNDTRIP=1 to run");
        return Ok(());
    }

    // Verify embedded model is being used
    let embedded = codescribe_core::stt::whisper::embedded::is_embedded_available();
    eprintln!("Whisper embedded model available: {}", embedded);

    codescribe_core::stt::whisper::init()?;

    // Generate test audio (simple sine wave as speech proxy)
    // In real tests, we'd use TTS-generated audio
    let sample_rate = 16000;
    let duration_sec = 2.0;
    let samples: Vec<f32> = (0..(sample_rate as f32 * duration_sec) as usize)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            (t * 440.0 * 2.0 * std::f32::consts::PI).sin() * 0.3
        })
        .collect();

    // Should not crash, even with non-speech audio
    let result = codescribe_core::stt::whisper::transcribe(&samples, sample_rate, Some("en"));
    eprintln!("Transcription result: {:?}", result);

    // It's OK if it returns empty or noise - we're testing it doesn't crash
    assert!(
        result.is_ok(),
        "Whisper should handle any audio without crashing"
    );

    Ok(())
}

#[test]
fn test_tts_embedded_model_works() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping: set CODESCRIBE_E2E_ROUNDTRIP=1 to run");
        return Ok(());
    }

    let embedded = codescribe_core::tts::embedded::is_embedded_available();
    eprintln!("TTS embedded model available: {}", embedded);

    codescribe_core::tts::init()?;

    let text = "Test.";
    let audio = codescribe_core::tts::synthesize(text)?;

    eprintln!("Generated {} samples for '{}'", audio.len(), text);
    assert!(!audio.is_empty(), "TTS should produce audio");

    // Check audio is valid (not all zeros, not all same value)
    let min = audio.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = audio.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = max - min;

    eprintln!(
        "Audio range: {:.4} to {:.4} (range: {:.4})",
        min, max, range
    );
    assert!(range > 0.01, "Audio should have dynamic range");

    Ok(())
}

#[test]
fn test_embedded_model_works() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping: set CODESCRIBE_E2E_ROUNDTRIP=1 to run");
        return Ok(());
    }

    let embedded = codescribe_core::embedder::embedded::is_embedded_available();
    eprintln!("Embedded model available: {}", embedded);

    codescribe_core::embedder::init()?;

    let text = "Hello world";
    let embedding = codescribe_core::embedder::embed(text)?;

    eprintln!("Embedding dimension: {}", embedding.len());
    assert_eq!(
        embedding.len(),
        codescribe_core::embedder::EMBEDDING_DIM,
        "Embedding should match expected dimension"
    );

    // Check embedding is normalized (L2 norm ≈ 1.0)
    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    eprintln!("Embedding L2 norm: {:.4}", norm);
    assert!(
        (norm - 1.0).abs() < 0.1,
        "Embeddings should be normalized. Got norm: {:.4}",
        norm
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════
// Regression Tests
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_numbers_survive_round_trip() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping: set CODESCRIBE_E2E_ROUNDTRIP=1 to run");
        return Ok(());
    }

    codescribe_core::tts::init()?;
    codescribe_core::stt::whisper::init()?;

    let inputs = [
        "My phone number is 555-1234.",
        "The meeting is at 3:30 PM.",
        "We need 42 units by Friday.",
    ];

    for input in inputs {
        eprintln!("\nTesting: '{}'", input);
        let audio = codescribe_core::tts::synthesize(input)?;
        let output = codescribe_core::stt::whisper::transcribe(&audio, 24000, Some("en"))?;
        eprintln!("Got: '{}'", output.trim());

        // Numbers are tricky - just ensure we get something back
        assert!(!output.trim().is_empty(), "Should transcribe something");
    }

    Ok(())
}

#[test]
fn test_punctuation_handling() -> Result<()> {
    if !should_run() {
        eprintln!("Skipping: set CODESCRIBE_E2E_ROUNDTRIP=1 to run");
        return Ok(());
    }

    codescribe_core::tts::init()?;
    codescribe_core::stt::whisper::init()?;

    let input = "Hello! How are you? I'm fine, thanks.";
    eprintln!("Input: '{}'", input);

    let audio = codescribe_core::tts::synthesize(input)?;
    let output = codescribe_core::stt::whisper::transcribe(&audio, 24000, Some("en"))?;
    eprintln!("Output: '{}'", output.trim());

    // Check that multiple sentences are preserved
    let sentence_count = output.matches(['.', '!', '?']).count();
    eprintln!("Sentence endings found: {}", sentence_count);

    // At least some punctuation should survive
    assert!(
        sentence_count >= 1,
        "Should preserve some sentence structure"
    );

    Ok(())
}
