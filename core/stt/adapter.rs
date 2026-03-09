//! STT adapter — bridges the Whisper singleton to the `TranscriptionAdapter` contract.
//!
//! This is a thin wrapper: no logic changes, just type translation.
//! Future providers (cloud STT, etc.) would implement the same trait.
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::Result;

use crate::pipeline::contracts::{RawTranscript, SpeechUtterance, TranscriptionAdapter};

/// Adapter wrapping the global Whisper singleton.
///
/// Uses `whisper::transcribe(samples, sample_rate, language)` under the hood.
/// Thread-safe: the singleton uses `OnceLock<Mutex<LocalWhisperEngine>>`.
pub struct WhisperSingletonAdapter;

impl Default for WhisperSingletonAdapter {
    fn default() -> Self {
        Self
    }
}

impl WhisperSingletonAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl TranscriptionAdapter for WhisperSingletonAdapter {
    fn transcribe(
        &self,
        utterance: &SpeechUtterance,
        language: Option<&str>,
    ) -> Result<RawTranscript> {
        crate::stt::whisper::transcribe_with_segments(
            &utterance.samples,
            utterance.sample_rate,
            language,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the adapter satisfies Send + Sync (required by trait bound).
    #[test]
    fn adapter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WhisperSingletonAdapter>();
    }

    /// Verify construction doesn't panic.
    #[test]
    fn adapter_construction() {
        let _adapter = WhisperSingletonAdapter::new();
    }

    /// Test with a mock: any struct implementing TranscriptionAdapter works.
    struct MockAdapter {
        response: String,
    }

    impl TranscriptionAdapter for MockAdapter {
        fn transcribe(
            &self,
            _utterance: &SpeechUtterance,
            _language: Option<&str>,
        ) -> Result<RawTranscript> {
            Ok(RawTranscript {
                text: self.response.clone(),
                segments: Vec::new(),
            })
        }
    }

    #[test]
    fn mock_adapter_returns_text() {
        let adapter = MockAdapter {
            response: "Cześć, to jest test".into(),
        };
        let utterance = SpeechUtterance {
            samples: vec![0.0; 16000],
            sample_rate: 16000,
            start_ts: 0.0,
            end_ts: 1.0,
        };
        let result = adapter.transcribe(&utterance, Some("pl")).unwrap();
        assert_eq!(result.text, "Cześć, to jest test");
        assert!(result.segments.is_empty());
    }

    /// Verify trait object works (dyn dispatch).
    #[test]
    fn adapter_as_trait_object() {
        let adapter: Box<dyn TranscriptionAdapter> = Box::new(MockAdapter {
            response: "hello".into(),
        });
        let utterance = SpeechUtterance {
            samples: vec![0.0; 8000],
            sample_rate: 16000,
            start_ts: 0.0,
            end_ts: 0.5,
        };
        let result = adapter.transcribe(&utterance, None).unwrap();
        assert_eq!(result.text, "hello");
    }
}
