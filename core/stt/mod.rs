pub mod adapter;
pub mod onnx_adapter;
pub mod whisper;

use crate::pipeline::contracts::TranscriptionAdapter;

/// Get the active STT adapter based on `CODESCRIBE_STT_ENGINE` env var.
///
/// - `"onnx"` → initializes ONNX engine + returns `OnnxWhisperAdapter`
/// - anything else → `WhisperSingletonAdapter` (candle, default)
///
/// For ONNX, this calls `onnx_adapter::init()` automatically on first use.
/// Returns an error if the ONNX model is not available.
pub fn get_adapter() -> anyhow::Result<Box<dyn TranscriptionAdapter>> {
    match std::env::var("CODESCRIBE_STT_ENGINE").as_deref() {
        Ok("onnx") => {
            onnx_adapter::init()?;
            Ok(Box::new(onnx_adapter::OnnxWhisperAdapter::new()))
        }
        _ => Ok(Box::new(adapter::WhisperSingletonAdapter::new())),
    }
}

// ── Engine-level router ──────────────────────────────────────────────────────
//
// These functions dispatch to either candle or ONNX engine based on
// `CODESCRIBE_STT_ENGINE` env var. They match the call semantics of
// `LocalWhisperEngine::transcribe_with_language` (chunk) and
// `transcribe_long_with_language` (utterance/correction with try_lock).
//
// Used by `pipeline::streaming` to make the dual-engine switch transparent.

fn is_onnx_engine() -> bool {
    std::env::var("CODESCRIBE_STT_ENGINE").as_deref() == Ok("onnx")
}

/// Initialize whichever STT engine is active by env.
pub(crate) fn init_active_engine() -> anyhow::Result<()> {
    if is_onnx_engine() {
        onnx_adapter::init()
    } else {
        whisper::init()
    }
}

/// Transcribe long audio (blocking lock on whichever engine is active).
pub(crate) fn transcribe_long(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<String> {
    if is_onnx_engine() {
        onnx_adapter::transcribe_long(audio, sample_rate, language)
    } else {
        whisper::transcribe(audio, sample_rate, language)
    }
}

/// Transcribe a single chunk (blocking lock on whichever engine is active).
pub(crate) fn transcribe_chunk(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<String> {
    if is_onnx_engine() {
        onnx_adapter::transcribe_chunk(audio, sample_rate, language)
    } else {
        let engine = whisper::singleton::engine()?;
        let mut guard = engine
            .lock()
            .map_err(|e| anyhow::anyhow!("Candle lock error: {}", e))?;
        guard.transcribe_with_language(audio, sample_rate, language)
    }
}

/// Transcribe long audio (try_lock — returns error if engine is busy).
pub(crate) fn try_transcribe_long(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<String> {
    if is_onnx_engine() {
        onnx_adapter::try_transcribe_long(audio, sample_rate, language)
    } else {
        let engine = whisper::singleton::engine()?;
        let mut guard = engine
            .try_lock()
            .map_err(|_| anyhow::anyhow!("Whisper engine busy, skipping correction"))?;
        guard.transcribe_long_with_language(audio, sample_rate, language)
    }
}
