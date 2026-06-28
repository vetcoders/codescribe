//! E2E test for streaming post-processing with real microphone input.
//!
//! Run with:
//!   CODESCRIBE_E2E_MIC=1 cargo test --test e2e_stream_postprocess_mic -- --nocapture
//!
//! Optional:
//!   CODESCRIBE_E2E_MIC_LANGUAGE=en
//!
//! Created by Vetcoders (c)2026

use std::sync::Arc;
use std::time::Duration;

use codescribe::audio::streaming_recorder::StreamingRecorder;
use codescribe_core::pipeline::contracts::{EngineEvent, EventSink};
use codescribe_core::pipeline::sinks::CollectorEventSink;

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
    let sink = Arc::new(CollectorEventSink::new());
    recorder.set_event_sink(Some(Arc::clone(&sink) as Arc<dyn EventSink>));

    eprintln!("Speak for ~6s. Suggested phrase: 'Docker GitHub API key'.");
    recorder
        .start_event_session(language)
        .await
        .expect("Failed to start streaming recorder");

    tokio::time::sleep(Duration::from_secs(6)).await;

    let (_legacy_transcript, _audio_path) = recorder.stop().await.expect("Failed to stop recorder");
    let events = sink.events();
    let transcript = transcript_from_events(&events);

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

fn transcript_from_events(events: &[EngineEvent]) -> String {
    let mut finalized = Vec::new();
    let mut preview = String::new();

    for event in events {
        match event {
            EngineEvent::Preview { text, .. } | EngineEvent::Correction { text, .. } => {
                preview = text.clone();
            }
            EngineEvent::UtteranceFinal { text, .. } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    finalized.push(trimmed.to_string());
                }
                preview.clear();
            }
            EngineEvent::NoSpeech { .. } => {
                preview.clear();
            }
            _ => {}
        }
    }

    let preview = preview.trim();
    if !preview.is_empty() {
        finalized.push(preview.to_string());
    }

    finalized.join(" ")
}
