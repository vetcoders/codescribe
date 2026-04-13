//! E2E test for STT transcription using local Whisper engine
//!
//! Adapted from examples/e2e_stt.rs to use test assets and proper test structure.
//!
//! To run (requires model):
//!   CODESCRIBE_E2E_STT=1 cargo test --test e2e_stt_transcription
//!
//! Created by M&K (c)2026 VetCoders

use std::path::{Path, PathBuf};

use codescribe::whisper::LocalWhisperEngine;
use codescribe_core::pipeline::contracts::FileTranscriptionOptions;
use tempfile::TempDir;

#[path = "support/e2e_stt_matrix.rs"]
mod e2e_stt_matrix;

use e2e_stt_matrix::{
    ModelDiscovery, ModelSource, STT_OPT_IN_ENV, WHISPER_LARGE_MODEL, WHISPER_TURBO_MODEL,
    discover_local_whisper_model, discover_local_whisper_model_for, model_discovery_hint,
    parse_opt_in, skip_unless_opt_in, test_audio_path, whisper_model_missing_parts,
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

/// Full STT E2E test with local Whisper engine
///
/// Run with: CODESCRIBE_E2E_STT=1 cargo test --test e2e_stt_transcription
#[test]
fn e2e_stt_transcribe_test_audio() {
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "STT transcription E2E",
        "Deterministic discovery/gating checks still run by default.",
    ) {
        return;
    }

    let found = match resolve_model_or_skip("STT transcription E2E") {
        Some(found) => found,
        None => return,
    };

    println!("═══════════════════════════════════════════════════════════");
    println!("  Local Whisper STT E2E Test");
    println!("═══════════════════════════════════════════════════════════");
    println!("  Model: {} ({:?})", found.path.display(), found.source);

    // Initialize engine
    println!("  Loading model...");
    let start = std::time::Instant::now();
    let mut engine = LocalWhisperEngine::new(&found.path).expect("load model");
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
    let verdict = engine
        .transcribe_file_with_language(
            &audio_path,
            Some(&language),
            FileTranscriptionOptions::default(),
        )
        .expect("transcribe");
    let text = verdict.text;
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
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "STT language detection E2E",
        "Set CODESCRIBE_E2E_STT=1 when local model tests are needed.",
    ) {
        return;
    }

    let found = match resolve_model_or_skip("STT language detection E2E") {
        Some(found) => found,
        None => return,
    };

    let mut engine = LocalWhisperEngine::new(&found.path).expect("load model");
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
    if skip_unless_opt_in(
        STT_OPT_IN_ENV,
        "STT model init stability E2E",
        "Set CODESCRIBE_E2E_STT=1 to run model-loading checks.",
    ) {
        return;
    }

    let found = match resolve_model_or_skip("STT model init stability E2E") {
        Some(found) => found,
        None => return,
    };

    // Initialize twice - should not panic
    let engine1 = LocalWhisperEngine::new(&found.path);
    assert!(engine1.is_ok(), "First init failed");

    let engine2 = LocalWhisperEngine::new(&found.path);
    assert!(engine2.is_ok(), "Second init failed");

    println!("Model initialization is stable (can be called multiple times)");
}

fn create_complete_model(path: &Path) {
    std::fs::create_dir_all(path).expect("create model dir");
    std::fs::write(path.join("config.json"), "{}").expect("write config");
    std::fs::write(path.join("tokenizer.json"), "{}").expect("write tokenizer");
    std::fs::write(path.join("mel_filters.npz"), []).expect("write mel filters");
    std::fs::write(path.join("weights.safetensors"), []).expect("write weights");
}

fn create_incomplete_model(path: &Path) {
    std::fs::create_dir_all(path).expect("create model dir");
    std::fs::write(path.join("tokenizer.json"), "{}").expect("write tokenizer");
}

fn temp_home() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("create temp dir");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).expect("create temp home");
    (tmp, home)
}

#[test]
fn deterministic_gate_parser_requires_explicit_opt_in_values() {
    assert!(parse_opt_in(Some("1")), "expected 1 to enable opt-in gate");
    assert!(
        parse_opt_in(Some("true")),
        "expected true to enable opt-in gate"
    );
    assert!(
        parse_opt_in(Some("TRUE")),
        "expected TRUE to enable opt-in gate"
    );

    assert!(!parse_opt_in(Some("0")), "0 must not enable opt-in gate");
    assert!(
        !parse_opt_in(Some("false")),
        "false must not enable opt-in gate"
    );
    assert!(
        !parse_opt_in(Some("yes")),
        "yes must not enable opt-in gate"
    );
    assert!(
        !parse_opt_in(None),
        "missing env var must not enable opt-in gate"
    );
}

#[test]
fn deterministic_model_discovery_prefers_complete_env_override() {
    let (_tmp, home) = temp_home();
    let models_root = home.join(".codescribe/models");
    let turbo = models_root.join(WHISPER_TURBO_MODEL);
    let env_model = home.join("custom/whisper-model");

    create_complete_model(&turbo);
    create_complete_model(&env_model);

    let hf_bases = Vec::<PathBuf>::new();
    let found = discover_local_whisper_model_for(&home, Some(&env_model), &hf_bases)
        .expect("expected env override to be discovered");

    assert_eq!(
        found.source,
        ModelSource::EnvOverride,
        "env override must win over standard ~/.codescribe models"
    );
    assert_eq!(
        found.path, env_model,
        "discovered path should match CODESCRIBE_MODEL_PATH candidate"
    );
}

#[test]
fn deterministic_model_discovery_skips_incomplete_turbo_and_falls_back_to_large() {
    let (_tmp, home) = temp_home();
    let models_root = home.join(".codescribe/models");
    let turbo = models_root.join(WHISPER_TURBO_MODEL);
    let large = models_root.join(WHISPER_LARGE_MODEL);

    create_incomplete_model(&turbo);
    create_complete_model(&large);

    let hf_bases = Vec::<PathBuf>::new();
    let found = discover_local_whisper_model_for(&home, None, &hf_bases)
        .expect("expected fallback to large model");

    assert_eq!(
        found.source,
        ModelSource::UserLarge,
        "incomplete turbo model must not block fallback to complete large model"
    );
    assert_eq!(
        found.path, large,
        "expected large model path to be selected"
    );

    let missing = whisper_model_missing_parts(&turbo);
    assert!(
        missing.contains(&"config.json"),
        "incomplete turbo should report missing artifacts for easier diagnosis"
    );
}
