//! E2E test for STT transcription using local Whisper engine
//!
//! Adapted from examples/e2e_stt.rs to use test assets and proper test structure.
//!
//! To run (requires model):
//!   CODESCRIBE_E2E_STT=1 cargo test --test e2e_stt_transcription
//!
//! Created by Vetcoders (c)2026

use std::path::{Path, PathBuf};

use codescribe::whisper::LocalWhisperEngine;
use codescribe_core::pipeline::contracts::FileTranscriptionOptions;
use tempfile::TempDir;

#[path = "support/e2e_stt_matrix.rs"]
mod e2e_stt_matrix;

use e2e_stt_matrix::{
    ModelDiscovery, ModelSource, STT_OPT_IN_ENV, WHISPER_FP16_MODEL, discover_local_whisper_model,
    discover_local_whisper_model_for, discover_local_whisper_model_for_with_root,
    model_discovery_hint, parse_opt_in, skip_unless_opt_in, test_audio_path,
    whisper_model_missing_parts,
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
    std::fs::write(
        path.join("config.json"),
        include_str!("fixtures/whisper_config.json"),
    )
    .expect("write config");
    let mut tokenizer = tokenizers::Tokenizer::new(tokenizers::models::bpe::BPE::default());
    tokenizer.add_special_tokens(&[
        tokenizers::AddedToken::from("<|startoftranscript|>", true),
        tokenizers::AddedToken::from("<|endoftext|>", true),
    ]);
    tokenizer
        .save(path.join("tokenizer.json"), false)
        .expect("write tokenizer");
    std::fs::write(
        path.join("mel_filters.npz"),
        decode_hex(include_str!("fixtures/whisper_mel_filters.npz.hex")),
    )
    .expect("write mel filters");
    let header = br#"{"model.weight":{"dtype":"F16","shape":[1],"data_offsets":[0,2]}}"#;
    let mut safetensors = (header.len() as u64).to_le_bytes().to_vec();
    safetensors.extend_from_slice(header);
    safetensors.extend_from_slice(&[0, 0]);
    std::fs::write(path.join("weights.safetensors"), safetensors).expect("write weights");
}

fn decode_hex(raw: &str) -> Vec<u8> {
    let digits: String = raw.chars().filter(|ch| !ch.is_whitespace()).collect();
    assert!(digits.len().is_multiple_of(2));
    digits
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
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
fn deterministic_model_discovery_hint_names_the_validation_contract() {
    let hint = model_discovery_hint(Path::new("/tmp/test-home"));
    assert!(hint.contains("parseable config and tokenizer"));
    assert!(hint.contains("pinned mel_filters.npz checksum"));
    assert!(hint.contains("structurally valid F16/F32 safetensors"));
    assert!(hint.contains("no quantization declaration"));
}

#[test]
fn deterministic_model_discovery_prefers_complete_env_override() {
    let (_tmp, home) = temp_home();
    let models_root = home.join(".codescribe/models");
    let fp16 = models_root.join(WHISPER_FP16_MODEL);
    let env_model = home.join("custom/whisper-model");

    create_complete_model(&fp16);
    create_complete_model(&env_model);

    let found = discover_local_whisper_model_for(&home, Some(&env_model))
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
fn deterministic_model_discovery_skips_invalid_env_override() {
    let (_tmp, home) = temp_home();
    let models_root = home.join(".codescribe/models");
    let fp16 = models_root.join(WHISPER_FP16_MODEL);
    let env_model = home.join("custom/quantized-whisper-model");

    create_complete_model(&fp16);
    create_complete_model(&env_model);
    std::fs::write(
        env_model.join("config.json"),
        r#"{"quantization":{"bits":8}}"#,
    )
    .expect("mark env override as quantized");

    let found = discover_local_whisper_model_for(&home, Some(&env_model))
        .expect("expected valid standard fp16 model to be discovered");

    assert_eq!(found.source, ModelSource::ModelsDir);
    assert_eq!(found.path, fp16);
}

#[test]
fn deterministic_model_discovery_honors_existing_custom_models_root() {
    let (_tmp, home) = temp_home();
    let custom_root = home.join("custom-models");
    let fp16 = custom_root.join(WHISPER_FP16_MODEL);
    create_complete_model(&fp16);

    let found = discover_local_whisper_model_for_with_root(&home, None, Some(&custom_root))
        .expect("expected model under CODESCRIBE_MODELS_DIR");

    assert_eq!(found.source, ModelSource::ModelsDir);
    assert_eq!(found.path, fp16);
}

#[test]
fn deterministic_existing_empty_models_root_shadows_home_fallback() {
    let (_tmp, home) = temp_home();
    let home_fp16 = home.join(".codescribe/models").join(WHISPER_FP16_MODEL);
    create_complete_model(&home_fp16);
    let custom_root = home.join("empty-custom-models");
    std::fs::create_dir_all(&custom_root).unwrap();

    assert!(
        discover_local_whisper_model_for_with_root(&home, None, Some(&custom_root)).is_none(),
        "an existing explicit models root must own discovery even when empty"
    );
}

#[test]
fn deterministic_missing_models_root_falls_back_to_home() {
    let (_tmp, home) = temp_home();
    let home_fp16 = home.join(".codescribe/models").join(WHISPER_FP16_MODEL);
    create_complete_model(&home_fp16);
    let missing_root = home.join("missing-custom-models");

    let found = discover_local_whisper_model_for_with_root(&home, None, Some(&missing_root))
        .expect("missing override root should preserve runtime home fallback");

    assert_eq!(found.source, ModelSource::ModelsDir);
    assert_eq!(found.path, home_fp16);
}

#[test]
fn deterministic_model_discovery_refuses_incomplete_fp16_without_legacy_fallback() {
    let (_tmp, home) = temp_home();
    let models_root = home.join(".codescribe/models");
    let fp16 = models_root.join(WHISPER_FP16_MODEL);

    create_incomplete_model(&fp16);

    assert!(
        discover_local_whisper_model_for(&home, None).is_none(),
        "an incomplete fp16 model must not fall back to a quantized model"
    );

    let missing = whisper_model_missing_parts(&fp16);
    assert!(
        missing.contains(&"config.json"),
        "incomplete fp16 should report missing artifacts for easier diagnosis"
    );
}
