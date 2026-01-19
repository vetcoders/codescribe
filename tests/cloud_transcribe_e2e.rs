//! E2E test for cloud transcription (requires real API credentials).
//!
//! Enable with:
//!   CODESCRIBE_E2E_CLOUD=1 STT_ENDPOINT=... STT_API_KEY=... cargo test --test cloud_transcribe_e2e

use std::path::PathBuf;

#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_cloud_transcribe_e2e() {
    if !env_bool("CODESCRIBE_E2E_CLOUD") {
        eprintln!("Skipping cloud E2E (set CODESCRIBE_E2E_CLOUD=1 to enable)");
        return;
    }

    if std::env::var("STT_ENDPOINT").is_err() || std::env::var("STT_API_KEY").is_err() {
        eprintln!("Skipping cloud E2E (STT_ENDPOINT/STT_API_KEY missing)");
        return;
    }

    let audio = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/1.fretka-Ziggy.mp3");
    assert!(audio.exists(), "Missing test audio at {}", audio.display());

    let text = codescribe::client::transcribe(&audio, None)
        .await
        .expect("Cloud transcription failed");
    assert!(
        !text.trim().is_empty(),
        "Cloud transcription returned empty text"
    );
}

fn env_bool(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}
