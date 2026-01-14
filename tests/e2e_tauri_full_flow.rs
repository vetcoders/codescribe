//! E2E tests for full Tauri pipeline: Audio → Transcription → Formatting
//!
//! Simulates the complete flow that happens when user records audio in Tauri UI.
//!
//! To run full tests:
//!   CODESCRIBE_E2E_STT=1 cargo test --test e2e_tauri_full_flow
//!
//! To run with formatting (requires LLM):
//!   CODESCRIBE_E2E_STT=1 CODESCRIBE_E2E_FORMATTING=1 \
//!   LLM_HOST=... LLM_MODEL=... cargo test --test e2e_tauri_full_flow
//!
//! Created by M&K (c)2026 VetCoders

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use codescribe::{ai_formatting, audio, state::history};
use mockito::Matcher;
use serial_test::serial;
use tempfile::TempDir;

/// Path to synthetic test audio file
fn test_audio_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/1.fretka-Ziggy.mp3")
}

/// Find Whisper model path
fn find_model_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let model_candidates = [
        PathBuf::from(&home).join(".CodeScribe/models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from("models/whisper-large-v3-turbo-mlx-q8"),
        PathBuf::from("models/whisper-large-v3-mlx-q8"),
    ];

    model_candidates
        .iter()
        .find(|p| p.join("tokenizer.json").exists())
        .cloned()
}

/// Full pipeline: Audio → Load → Transcribe → Save to history
///
/// This simulates what happens in Tauri when user stops recording.
#[test]
#[serial]
fn test_full_pipeline_audio_to_history() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping full pipeline E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    let model_path = match find_model_path() {
        Some(p) => p,
        None => {
            eprintln!("No model found, skipping test");
            return;
        }
    };

    // Use temp dir for history
    let tmp = TempDir::new().expect("tempdir");
    unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path()) };

    // 1. Load audio (simulates what happens after recording stops)
    let audio_path = test_audio_path();
    let (samples, sample_rate) = audio::load_audio_file(&audio_path).expect("load audio");

    println!(
        "Step 1: Loaded audio - {} samples @ {} Hz",
        samples.len(),
        sample_rate
    );

    // 2. Transcribe
    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");
    let text = engine
        .transcribe_long_with_language(&samples, sample_rate, Some("pl"))
        .expect("transcribe");

    println!("Step 2: Transcribed - {} chars", text.len());
    assert!(!text.is_empty(), "Transcription should not be empty");

    // 3. Save to history
    let entry = history::save_entry(&text);
    assert!(entry.path.exists(), "History entry should be saved");
    assert!(entry.path.starts_with(tmp.path()), "Should use temp dir");

    println!("Step 3: Saved to history - {}", entry.path.display());

    // 4. Verify can read back
    let content = std::fs::read_to_string(&entry.path).expect("read history");
    assert_eq!(content.trim(), text.trim(), "History content should match");

    // 5. Verify latest_entry works
    let latest = history::latest_entry().expect("latest entry");
    assert_eq!(latest.path, entry.path, "Latest should match saved");

    println!("Step 4: Verified history read-back");
}

/// Full pipeline with streaming: Audio → Load → Stream Transcribe → Callbacks
#[test]
#[serial]
fn test_full_pipeline_with_streaming() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping streaming pipeline E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    let model_path = match find_model_path() {
        Some(p) => p,
        None => {
            eprintln!("No model found, skipping test");
            return;
        }
    };

    let audio_path = test_audio_path();
    let (samples, sample_rate) = audio::load_audio_file(&audio_path).expect("load audio");

    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");

    // Simulate Tauri event emission
    let events_emitted = Arc::new(AtomicUsize::new(0));
    let events_clone = Arc::clone(&events_emitted);
    let last_preview = Arc::new(std::sync::Mutex::new(String::new()));
    let preview_clone = Arc::clone(&last_preview);

    let callback = move |text: &str| {
        events_clone.fetch_add(1, Ordering::SeqCst);
        *preview_clone.lock().unwrap() = text.to_string();
        // In real Tauri: app.emit("transcript_chunk", text)
    };

    let final_text = engine
        .transcribe_long_streaming(&samples, sample_rate, Some("pl"), Some(&callback))
        .expect("transcribe streaming");

    let events = events_emitted.load(Ordering::SeqCst);
    let preview = last_preview.lock().unwrap().clone();

    println!("Streaming completed:");
    println!("  - Events emitted: {}", events);
    println!("  - Final text: {} chars", final_text.len());
    println!("  - Last preview: {} chars", preview.len());

    // Verify final text matches last preview
    assert_eq!(
        final_text.trim(),
        preview.trim(),
        "Final should match last preview"
    );
}

/// Full pipeline with mocked formatting
#[tokio::test]
#[serial]
async fn test_full_pipeline_with_formatting_mocked() {
    let enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping formatting pipeline E2E (set CODESCRIBE_E2E_STT=1 to enable)");
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    let model_path = match find_model_path() {
        Some(p) => p,
        None => {
            eprintln!("No model found, skipping test");
            return;
        }
    };

    let tmp = TempDir::new().expect("tempdir");
    unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path()) };

    // 1. Transcribe
    let audio_path = test_audio_path();
    let (samples, sample_rate) = audio::load_audio_file(&audio_path).expect("load audio");

    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");
    let raw_text = engine
        .transcribe_long_with_language(&samples, sample_rate, Some("pl"))
        .expect("transcribe");

    println!("Raw transcription: {} chars", raw_text.len());

    // 2. Mock LLM for formatting
    let mut server = mockito::Server::new();
    let endpoint = format!("{}/v1/responses", server.url());

    unsafe {
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "0");
        std::env::set_var("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "2000");
        std::env::set_var("LLM_HOST", &endpoint);
        std::env::set_var("LLM_MODEL", "test-model");
        std::env::set_var("LLM_API_KEY", "test-key");
    }

    // Mock returns formatted version (uppercase + period)
    let _m = server
        .mock("POST", "/v1/responses")
        .match_body(Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"id":"resp_test","output":[{{"type":"message","content":[{{"type":"output_text","text":"{}"}}]}}]}}"#,
            raw_text.to_uppercase().replace('\n', " ").trim()
        ))
        .create();

    // 3. Format
    let formatted = ai_formatting::format_text(&raw_text, Some("pl"), false).await;

    println!("Formatted: {} chars", formatted.len());
    assert!(!formatted.is_empty(), "Formatted should not be empty");

    // 4. Save both to history
    let raw_entry = history::save_entry(&raw_text);
    let formatted_entry = history::save_entry(&formatted);

    assert_ne!(
        raw_entry.path, formatted_entry.path,
        "Should be separate entries"
    );

    println!("Saved raw: {}", raw_entry.path.display());
    println!("Saved formatted: {}", formatted_entry.path.display());
}

/// Full pipeline with real formatting (opt-in)
#[tokio::test]
#[serial]
async fn test_full_pipeline_with_real_formatting() {
    let stt_enabled = std::env::var("CODESCRIBE_E2E_STT")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let formatting_enabled = std::env::var("CODESCRIBE_E2E_FORMATTING")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !stt_enabled || !formatting_enabled {
        eprintln!(
            "Skipping real formatting E2E (set CODESCRIBE_E2E_STT=1 CODESCRIBE_E2E_FORMATTING=1)"
        );
        return;
    }

    // Check LLM config
    let llm_host = std::env::var("LLM_HOST").ok();
    let llm_model = std::env::var("LLM_MODEL").ok();

    if llm_host.is_none() || llm_model.is_none() {
        eprintln!("Skipping: LLM_HOST and LLM_MODEL required");
        return;
    }

    use codescribe::whisper::LocalWhisperEngine;

    let model_path = match find_model_path() {
        Some(p) => p,
        None => {
            eprintln!("No model found, skipping test");
            return;
        }
    };

    let tmp = TempDir::new().expect("tempdir");
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path());
        std::env::set_var("CODESCRIBE_AI_MAX_RETRIES", "1");
        std::env::set_var("CODESCRIBE_AI_ATTEMPT_TIMEOUT_MS", "30000");
    }

    // 1. Transcribe
    let audio_path = test_audio_path();
    let (samples, sample_rate) = audio::load_audio_file(&audio_path).expect("load audio");

    let mut engine = LocalWhisperEngine::new(&model_path).expect("load model");
    let raw_text = engine
        .transcribe_long_with_language(&samples, sample_rate, Some("pl"))
        .expect("transcribe");

    println!("═══════════════════════════════════════════════════");
    println!("Raw transcription ({} chars):", raw_text.len());
    println!("───────────────────────────────────────────────────");
    println!("{}", &raw_text[..raw_text.len().min(300)]);
    if raw_text.len() > 300 {
        println!("... [{} more chars]", raw_text.len() - 300);
    }

    // 2. Format with real LLM
    let start = std::time::Instant::now();
    let formatted = ai_formatting::format_text(&raw_text, Some("pl"), false).await;
    let format_time = start.elapsed();

    println!("═══════════════════════════════════════════════════");
    println!("Formatted ({} chars, {:?}):", formatted.len(), format_time);
    println!("───────────────────────────────────────────────────");
    println!("{}", &formatted[..formatted.len().min(300)]);
    if formatted.len() > 300 {
        println!("... [{} more chars]", formatted.len() - 300);
    }
    println!("═══════════════════════════════════════════════════");

    // Basic assertions
    assert!(!formatted.is_empty(), "Formatted should not be empty");

    // Formatting should typically add punctuation, so length changes
    let delta = formatted.len() as i64 - raw_text.len() as i64;
    println!("Length delta: {} chars", delta);
}

/// Regression test: verify history entries are independent
#[test]
#[serial]
fn test_history_entries_independent() {
    let tmp = TempDir::new().expect("tempdir");
    unsafe { std::env::set_var("CODESCRIBE_DATA_DIR", tmp.path()) };

    let entry1 = history::save_entry("First transcription");
    let entry2 = history::save_entry("Second transcription");
    let entry3 = history::save_entry("Third transcription");

    // All should be separate files
    assert_ne!(entry1.path, entry2.path);
    assert_ne!(entry2.path, entry3.path);
    assert_ne!(entry1.path, entry3.path);

    // All should exist
    assert!(entry1.path.exists());
    assert!(entry2.path.exists());
    assert!(entry3.path.exists());

    // Content should be preserved
    assert!(std::fs::read_to_string(&entry1.path)
        .unwrap()
        .contains("First"));
    assert!(std::fs::read_to_string(&entry2.path)
        .unwrap()
        .contains("Second"));
    assert!(std::fs::read_to_string(&entry3.path)
        .unwrap()
        .contains("Third"));

    // Latest should be entry3
    let latest = history::latest_entry().expect("latest");
    assert_eq!(latest.path, entry3.path);
}
