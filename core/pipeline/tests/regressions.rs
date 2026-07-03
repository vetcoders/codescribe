use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};

use crate::audio::chunker::SpeechSession;
use crate::vad;

struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: tests run single-threaded with controlled env usage.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }

    fn unset(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: tests run single-threaded with controlled env usage.
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(prev) = &self.prev {
            // SAFETY: tests run single-threaded with controlled env usage.
            unsafe { std::env::set_var(self.key, prev) };
        } else {
            // SAFETY: tests run single-threaded with controlled env usage.
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("core crate should be nested under workspace root")
        .to_path_buf()
}

fn read_workspace_source(relative_path: &str) -> String {
    let path = workspace_root().join(relative_path);
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()))
}

#[test]
#[serial]
fn utterance_silence_default_regression() {
    // Ensure a clean baseline for this test (do not inherit user shell env).
    let _g = EnvGuard::unset("CODESCRIBE_BUFFERED_SILENCE_SEC");

    let sr = 16000u32;
    let stream = SpeechSession::new_stream(sr, 3.0, 0.6);
    let utterance = SpeechSession::new_utterance(sr);

    let base = vad::VadConfig::default();
    let stream_expected = (base.max_silence_duration_sec * vad::VAD_SAMPLE_RATE as f32)
        .round()
        .max(1.0) as usize;
    assert_eq!(stream.min_silence_samples(), stream_expected);

    // Utterance mode should keep VadConfig default silence unless explicitly overridden.
    let utter_expected = (base.max_silence_duration_sec * vad::VAD_SAMPLE_RATE as f32)
        .round()
        .max(1.0) as usize;
    assert_eq!(utterance.min_silence_samples(), utter_expected);
}

#[test]
#[serial]
fn utterance_silence_override_env_regression() {
    let _g = EnvGuard::set("CODESCRIBE_BUFFERED_SILENCE_SEC", "0.45");

    let sr = 16000u32;
    let stream = SpeechSession::new_stream(sr, 3.0, 0.6);
    let utterance = SpeechSession::new_utterance(sr);

    // Stream default should remain at the hardcoded Silero base.
    let base = vad::VadConfig::default();
    let stream_expected = (base.max_silence_duration_sec * vad::VAD_SAMPLE_RATE as f32)
        .round()
        .max(1.0) as usize;
    assert_eq!(stream.min_silence_samples(), stream_expected);

    // Utterance should respect the buffered/utterance-specific override.
    let utter_expected = (0.45 * vad::VAD_SAMPLE_RATE as f32).round().max(1.0) as usize;
    assert_eq!(utterance.min_silence_samples(), utter_expected);
}

#[test]
fn runtime_contract_blocks_legacy_delta_callback_api() {
    let source = read_workspace_source("core/audio/streaming_recorder.rs");

    assert!(
        !source.contains("set_delta_callback("),
        "legacy set_delta_callback API must stay removed"
    );
    assert!(
        source.contains("set_event_sink("),
        "runtime contract requires set_event_sink API"
    );
    assert!(
        source.contains("start_event_session("),
        "runtime contract requires start_event_session entrypoint"
    );
}

#[test]
fn runtime_contract_blocks_legacy_worker_symbols() {
    let banned_symbols = ["VadWorker", "LegacyVadWorker", "TranscriptionWorker"];

    let mut guarded_sources: Vec<(String, String)> = [
        "core/audio/streaming_recorder.rs",
        "app/controller/mod.rs",
        // Runtime entrypoint that installs the hotkey listener and hosts the
        // RecordingController for the SwiftUI app — successor to the removed
        // `bin/codescribe.rs` tray binary.
        "bridge/src/hotkeys.rs",
    ]
    .into_iter()
    .map(|relative_path| {
        (
            relative_path.to_string(),
            read_workspace_source(relative_path),
        )
    })
    .collect();

    // The streaming pipeline is a module directory after decomposition; guard
    // every Rust file inside it so new submodules stay covered automatically.
    let streaming_dir = workspace_root().join("core/pipeline/streaming");
    for entry in fs::read_dir(&streaming_dir).expect("streaming module directory must exist") {
        let path = entry.expect("readable streaming dir entry").path();
        if path.extension().is_some_and(|ext| ext == "rs") {
            let display = path.display().to_string();
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("failed to read {display}: {err}"));
            guarded_sources.push((display, source));
        }
    }

    for (name, source) in guarded_sources {
        for banned in banned_symbols {
            assert!(
                !source.contains(banned),
                "legacy worker symbol `{banned}` must not appear in {name}"
            );
        }
    }
}
