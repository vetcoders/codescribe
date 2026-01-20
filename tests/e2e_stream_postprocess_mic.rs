//! E2E test for streaming post-processing with real microphone input.
//!
//! Run with:
//!   CODESCRIBE_E2E_MIC=1 cargo test --test e2e_stream_postprocess_mic -- --nocapture
//!
//! Optional:
//!   CODESCRIBE_E2E_MIC_LANGUAGE=en
//!
//! Created by M&K (c)2026 VetCoders

use std::time::Duration;

use codescribe::audio::streaming_recorder::StreamingRecorder;

#[tokio::test]
async fn test_stream_postprocess_with_mic() {
    let enabled = std::env::var("CODESCRIBE_E2E_MIC")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    if !enabled {
        eprintln!("Skipping mic E2E (set CODESCRIBE_E2E_MIC=1 to enable)");
        return;
    }

    if std::env::var("CODESCRIBE_STREAM_FORCE_EMBEDDINGS").is_err() {
        eprintln!(
            "Embeddings are disabled in tests by default. Set CODESCRIBE_STREAM_FORCE_EMBEDDINGS=1 to enable."
        );
    }

    let language = std::env::var("CODESCRIBE_E2E_MIC_LANGUAGE").ok();

    codescribe::whisper::init().expect("Failed to init Whisper");

    let mut recorder = StreamingRecorder::new().expect("Failed to init streaming recorder");

    eprintln!("Speak for ~6s. Suggested phrase: 'Docker GitHub API key'.");
    recorder
        .start(language)
        .await
        .expect("Failed to start streaming recorder");

    tokio::time::sleep(Duration::from_secs(6)).await;

    let (transcript, _audio_path) = recorder.stop().await.expect("Failed to stop recorder");

    println!("Transcript: {}", transcript);

    let trimmed = transcript.trim();
    assert!(
        !trimmed.is_empty(),
        "Expected non-empty transcript. Check mic permissions and input."
    );

    let lower = trimmed.to_lowercase();
    let has_keyword = ["docker", "github", "api"]
        .iter()
        .any(|k| lower.contains(k));
    assert!(
        has_keyword,
        "Expected at least one keyword (docker/github/api) in transcript. Got: {}",
        trimmed
    );
}
