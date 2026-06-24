pub mod adapter;
pub mod apple_stt;
pub mod onnx_adapter;
pub mod scheduler;
pub mod whisper;

use crate::pipeline::contracts::RawTranscript;
use crate::pipeline::contracts::TranscriptionAdapter;
use std::sync::OnceLock;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SttEngine {
    Candle,
    Onnx,
    Apple,
}

fn selected_engine() -> SttEngine {
    match std::env::var("CODESCRIBE_STT_ENGINE")
        .unwrap_or_else(|_| "candle".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "onnx" => SttEngine::Onnx,
        "apple" => SttEngine::Apple,
        _ => SttEngine::Candle,
    }
}

/// Get the active STT adapter based on `CODESCRIBE_STT_ENGINE` env var.
///
/// - `"onnx"` → initializes ONNX engine + returns `OnnxWhisperAdapter`
/// - `"apple"` → initializes SpeechAnalyzer bridge + returns Apple adapter
/// - anything else → `WhisperSingletonAdapter` (candle, default)
///
/// Apple path gracefully falls back to Candle if unavailable.
pub fn get_adapter() -> anyhow::Result<Box<dyn TranscriptionAdapter>> {
    match selected_engine() {
        SttEngine::Onnx => {
            onnx_adapter::init()?;
            Ok(Box::new(onnx_adapter::OnnxWhisperAdapter::new()))
        }
        SttEngine::Apple => run_apple_or_whisper(
            "get_adapter",
            || {
                apple_stt::init()?;
                Ok(Box::new(apple_stt::AppleSpeechAnalyzerAdapter::new())
                    as Box<dyn TranscriptionAdapter>)
            },
            || {
                Ok(Box::new(adapter::WhisperSingletonAdapter::new())
                    as Box<dyn TranscriptionAdapter>)
            },
        ),
        SttEngine::Candle => Ok(Box::new(adapter::WhisperSingletonAdapter::new())),
    }
}

// ── Engine-level router ──────────────────────────────────────────────────────
//
// These functions dispatch to candle, ONNX, or Apple SpeechAnalyzer based on
// `CODESCRIBE_STT_ENGINE`. They match the call semantics of
// `LocalWhisperEngine::transcribe_with_language` (chunk) and
// `transcribe_long_with_language` (utterance/correction).
//
// Used by `pipeline::streaming` to keep backend selection transparent.

fn warn_apple_fallback(context: &str, error: &anyhow::Error) {
    static WARNED: OnceLock<()> = OnceLock::new();
    WARNED.get_or_init(|| {
        warn!(
            "Apple STT requested but unavailable during {}: {}. Falling back to Candle Whisper.",
            context, error
        );
    });
}

fn run_apple_or_whisper<T>(
    context: &str,
    apple_path: impl FnOnce() -> anyhow::Result<T>,
    whisper_fallback: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    if !apple_stt::is_runtime_available() {
        let err = anyhow::anyhow!("SpeechAnalyzer runtime not available on this host");
        warn_apple_fallback(context, &err);
        return whisper_fallback();
    }

    match apple_path() {
        Ok(value) => Ok(value),
        Err(err) => {
            warn_apple_fallback(context, &err);
            whisper_fallback()
        }
    }
}

// FORGOTTEN-GEM(vc-prune 2026-06-10): parked code, intentionally kept —
// the whole synchronous one-shot transcription contract (transcribe_chunk /
// try_transcribe_long_with_segments across whisper/apple/onnx providers) is
// parked: runtime uses the scheduler+streaming path. Kept as the documented
// provider contract for CLI/batch revival; operator decides revive-or-delete.
#[allow(dead_code)]
fn candle_transcribe_chunk(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<String> {
    let engine = whisper::singleton::engine()?;
    let mut guard = engine
        .lock()
        .map_err(|e| anyhow::anyhow!("Candle lock error: {}", e))?;
    guard.transcribe_with_language(audio, sample_rate, language)
}

fn candle_transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    let engine = whisper::singleton::engine()?;
    let mut guard = engine
        .lock()
        .map_err(|e| anyhow::anyhow!("Candle lock error: {}", e))?;
    guard.transcribe_long_with_language_segments(audio, sample_rate, language)
}

#[allow(dead_code)]
fn candle_try_transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    let engine = whisper::singleton::engine()?;
    let mut guard = engine
        .try_lock()
        .map_err(|_| anyhow::anyhow!("Whisper engine busy, skipping correction"))?;
    guard.transcribe_long_with_language_segments(audio, sample_rate, language)
}

/// Initialize whichever STT engine is active by env.
pub(crate) fn init_active_engine() -> anyhow::Result<()> {
    match selected_engine() {
        SttEngine::Onnx => onnx_adapter::init(),
        SttEngine::Apple => {
            run_apple_or_whisper("init_active_engine", apple_stt::init, whisper::init)
        }
        SttEngine::Candle => whisper::init(),
    }
}

/// Transcribe a single chunk (blocking lock on whichever engine is active).
// FORGOTTEN-GEM(vc-prune 2026-06-10): see candle_transcribe_chunk note above.
#[allow(dead_code)]
pub(crate) fn transcribe_chunk(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<String> {
    match selected_engine() {
        SttEngine::Onnx => onnx_adapter::transcribe_chunk(audio, sample_rate, language),
        SttEngine::Apple => run_apple_or_whisper(
            "transcribe_chunk",
            || apple_stt::transcribe_chunk(audio, sample_rate, language),
            || candle_transcribe_chunk(audio, sample_rate, language),
        ),
        SttEngine::Candle => candle_transcribe_chunk(audio, sample_rate, language),
    }
}

/// Transcribe long audio (blocking lock) with segment-level timestamps.
pub(crate) fn transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    match selected_engine() {
        SttEngine::Onnx => {
            onnx_adapter::transcribe_long_with_segments(audio, sample_rate, language)
        }
        SttEngine::Apple => run_apple_or_whisper(
            "transcribe_long_with_segments",
            || apple_stt::transcribe_long_with_segments(audio, sample_rate, language),
            || candle_transcribe_long_with_segments(audio, sample_rate, language),
        ),
        SttEngine::Candle => candle_transcribe_long_with_segments(audio, sample_rate, language),
    }
}

/// Transcribe long audio (try_lock) with segment-level timestamps.
#[allow(dead_code)]
pub(crate) fn try_transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    match selected_engine() {
        SttEngine::Onnx => {
            onnx_adapter::try_transcribe_long_with_segments(audio, sample_rate, language)
        }
        SttEngine::Apple => run_apple_or_whisper(
            "try_transcribe_long_with_segments",
            || apple_stt::try_transcribe_long_with_segments(audio, sample_rate, language),
            || candle_try_transcribe_long_with_segments(audio, sample_rate, language),
        ),
        SttEngine::Candle => candle_try_transcribe_long_with_segments(audio, sample_rate, language),
    }
}
