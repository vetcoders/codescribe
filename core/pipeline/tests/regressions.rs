use serial_test::serial;

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

#[test]
#[serial]
fn utterance_silence_default_regression() {
    // Ensure a clean baseline for this test (do not inherit user shell env).
    let _g1 = EnvGuard::unset("CODESCRIBE_VAD_SILENCE_SEC");
    let _g2 = EnvGuard::unset("CODESCRIBE_VAD_MAX_SILENCE_SEC");
    let _g3 = EnvGuard::unset("CODESCRIBE_BUFFERED_SILENCE_SEC");
    let _g4 = EnvGuard::unset("CODESCRIBE_UTTERANCE_SILENCE_SEC");

    let sr = 16000u32;
    let stream = SpeechSession::new_stream(sr, 3.0, 0.6);
    let utterance = SpeechSession::new_utterance(sr);

    // Streaming uses a short silence tolerance by default (chunk boundary responsiveness).
    let stream_expected = (0.20 * vad::VAD_SAMPLE_RATE as f32).round().max(1.0) as usize;
    assert_eq!(stream.min_silence_samples(), stream_expected);

    // Utterance mode should keep VadConfig default silence unless explicitly overridden.
    let base = vad::VadConfig::default();
    let utter_expected = (base.max_silence_duration_sec * vad::VAD_SAMPLE_RATE as f32)
        .round()
        .max(1.0) as usize;
    assert_eq!(utterance.min_silence_samples(), utter_expected);
    assert!(
        utterance.min_silence_samples() >= stream.min_silence_samples(),
        "utterance silence should be >= stream silence by default"
    );
}

#[test]
#[serial]
fn utterance_silence_override_env_regression() {
    let _g1 = EnvGuard::unset("CODESCRIBE_VAD_SILENCE_SEC");
    let _g2 = EnvGuard::unset("CODESCRIBE_VAD_MAX_SILENCE_SEC");
    let _g3 = EnvGuard::set("CODESCRIBE_BUFFERED_SILENCE_SEC", "0.45");
    let _g4 = EnvGuard::unset("CODESCRIBE_UTTERANCE_SILENCE_SEC");

    let sr = 16000u32;
    let stream = SpeechSession::new_stream(sr, 3.0, 0.6);
    let utterance = SpeechSession::new_utterance(sr);

    // Stream default should remain the short streaming silence if global VAD silence isn't set.
    let stream_expected = (0.20 * vad::VAD_SAMPLE_RATE as f32).round().max(1.0) as usize;
    assert_eq!(stream.min_silence_samples(), stream_expected);

    // Utterance should respect the buffered/utterance-specific override.
    let utter_expected = (0.45 * vad::VAD_SAMPLE_RATE as f32).round().max(1.0) as usize;
    assert_eq!(utterance.min_silence_samples(), utter_expected);
}
