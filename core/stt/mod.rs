pub mod adapter;
pub mod apple_stt;
pub mod onnx_adapter;
pub mod scheduler;
pub mod tail_patcher;
pub mod whisper;

use crate::pipeline::contracts::RawTranscript;
use crate::pipeline::contracts::TranscriptionAdapter;
use std::sync::OnceLock;
use tracing::warn;

const ENV_STT_ENGINE: &str = "CODESCRIBE_STT_ENGINE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SttEngine {
    Candle,
    Onnx,
    Apple,
}

fn selected_engine() -> SttEngine {
    match std::env::var(ENV_STT_ENGINE) {
        Ok(value) => requested_engine(&value).unwrap_or_else(default_engine),
        Err(_) => default_engine(),
    }
}

fn requested_engine(value: &str) -> Option<SttEngine> {
    match value.trim().to_ascii_lowercase().as_str() {
        "onnx" => SttEngine::Onnx,
        "apple" => SttEngine::Apple,
        "candle" | "whisper" => SttEngine::Candle,
        "" | "auto" => return None,
        _ => SttEngine::Candle,
    }
    .into()
}

fn default_engine() -> SttEngine {
    // AUTO only selects Apple when the SpeechAnalyzer bridge is actually
    // launchable; otherwise the probe is wasted and the router silently falls
    // back to Candle anyway (a misleading "Apple" selector). Explicit
    // `CODESCRIBE_STT_ENGINE=apple` bypasses this and still probes + fails loudly.
    if apple_stt::is_runtime_available() && apple_stt::is_bridge_resolvable() {
        SttEngine::Apple
    } else {
        SttEngine::Candle
    }
}

/// Get the active STT adapter based on `CODESCRIBE_STT_ENGINE` env var or auto policy.
///
/// - `"onnx"` → initializes ONNX engine + returns `OnnxWhisperAdapter`
/// - `"apple"` → initializes SpeechAnalyzer bridge + returns Apple adapter
/// - unset/`"auto"` → Apple on supported macOS, otherwise Candle
/// - anything else → `WhisperSingletonAdapter` (candle)
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
// `CODESCRIBE_STT_ENGINE` plus the default auto policy. They match the call semantics of
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

fn warn_initial_prompt_unsupported(engine: &str) {
    static WARNED: OnceLock<()> = OnceLock::new();
    WARNED.get_or_init(|| {
        warn!(
            "STT initial_prompt is supported only by Candle Whisper; {} route will ignore it.",
            engine
        );
    });
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
    // Engine acquisition + idle-clock refresh + lazy (re)load live in the
    // singleton now, so it can unload Whisper when idle and reload on demand.
    whisper::singleton::transcribe_chunk(audio, sample_rate, language)
}

fn candle_transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    whisper::singleton::transcribe_with_segments(audio, sample_rate, language)
}

fn candle_transcribe_long_with_segments_with_initial_prompt(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
    initial_prompt: Option<String>,
) -> anyhow::Result<RawTranscript> {
    whisper::singleton::transcribe_with_segments_with_initial_prompt(
        audio,
        sample_rate,
        language,
        initial_prompt,
    )
}

pub(crate) fn whisper_tail_patch_transcribe(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    let (speech, _) = crate::vad::extract_speech(audio, sample_rate);
    if speech.is_empty() {
        return Ok(RawTranscript::default());
    }
    candle_transcribe_long_with_segments(&speech, sample_rate, language)
}

#[allow(dead_code)]
fn candle_try_transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    // Non-blocking acquisition: skip the correction pass if the engine is busy.
    whisper::singleton::try_transcribe_with_segments(audio, sample_rate, language)
}

/// Initialize whichever STT engine is active by env.
pub fn init_active_engine() -> anyhow::Result<()> {
    match selected_engine() {
        SttEngine::Onnx => onnx_adapter::init(),
        SttEngine::Apple => {
            run_apple_or_whisper("init_active_engine", apple_stt::init, whisper::init)
        }
        SttEngine::Candle => whisper::init(),
    }
}

/// Sample rate of the synthetic warmup buffer.
const WARMUP_SAMPLE_RATE: u32 = 16_000;

/// Prewarm the ACTIVE STT engine end-to-end so the first real dictation pays
/// neither model-load nor (for the Candle/Metal path) first-inference Metal
/// kernel-compilation latency, and (for the Apple path) neither the bridge
/// spawn nor the SpeechAnalyzer asset/probe readiness.
///
/// This is deliberately routed through the exact same `transcribe_long_with_segments`
/// path the live pipeline uses, so whichever engine actually serves transcripts
/// at runtime gets warmed: on macOS 26+ the router selects Apple SpeechAnalyzer
/// and transparently falls back to Candle when the bridge is unavailable
/// ([`run_apple_or_whisper`]). Warming the hardcoded Candle singleton alone (the
/// previous behaviour) missed the active engine whenever Apple routing won, and
/// even on the Candle path it only loaded weights without compiling kernels —
/// both leaving the first dictation cold.
///
/// Best-effort: the warmup transcription's result is intentionally discarded and
/// its errors are logged, never propagated, so a cold-path hiccup can never block
/// recording readiness. `init_active_engine` failures (e.g. no model on disk) are
/// surfaced so callers can log them.
pub fn prewarm_active_engine() -> anyhow::Result<()> {
    init_active_engine()?;

    // Push a short synthetic utterance through the real routing so the serving
    // engine compiles its kernels / spins up its bridge before the user dictates.
    let warmup = synthetic_warmup_audio();
    match transcribe_long_with_segments(&warmup, WARMUP_SAMPLE_RATE, Some("en")) {
        Ok(_) => tracing::info!("STT active-engine warmup inference complete"),
        Err(error) => {
            tracing::warn!("STT active-engine warmup inference failed (non-fatal): {error:#}")
        }
    }
    Ok(())
}

/// One second of very low-amplitude tone at 16 kHz. Non-silent (so the full
/// encoder+decoder path executes during warmup) yet quiet enough that it yields
/// no spurious transcript text.
fn synthetic_warmup_audio() -> Vec<f32> {
    let n = WARMUP_SAMPLE_RATE as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / WARMUP_SAMPLE_RATE as f32;
            0.0005 * (2.0 * std::f32::consts::PI * 220.0 * t).sin()
        })
        .collect()
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

/// Transcribe long audio while seeding Candle Whisper with a per-call domain
/// vocabulary prompt. Non-Candle engines keep their existing behavior.
pub(crate) fn transcribe_long_with_segments_with_initial_prompt(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
    initial_prompt: Option<String>,
) -> anyhow::Result<RawTranscript> {
    if initial_prompt.is_none() {
        return transcribe_long_with_segments(audio, sample_rate, language);
    }

    match selected_engine() {
        SttEngine::Onnx => {
            warn_initial_prompt_unsupported("ONNX");
            onnx_adapter::transcribe_long_with_segments(audio, sample_rate, language)
        }
        SttEngine::Apple => {
            let prompt_for_candle = initial_prompt.clone();
            run_apple_or_whisper(
                "transcribe_long_with_segments_with_initial_prompt",
                || {
                    warn_initial_prompt_unsupported("Apple SpeechAnalyzer");
                    apple_stt::transcribe_long_with_segments(audio, sample_rate, language)
                },
                || {
                    candle_transcribe_long_with_segments_with_initial_prompt(
                        audio,
                        sample_rate,
                        language,
                        prompt_for_candle,
                    )
                },
            )
        }
        SttEngine::Candle => candle_transcribe_long_with_segments_with_initial_prompt(
            audio,
            sample_rate,
            language,
            initial_prompt,
        ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvGuard {
        previous: Option<String>,
    }

    impl EnvGuard {
        fn unset() -> Self {
            let previous = std::env::var(ENV_STT_ENGINE).ok();
            unsafe { std::env::remove_var(ENV_STT_ENGINE) };
            Self { previous }
        }

        fn set(value: &str) -> Self {
            let previous = std::env::var(ENV_STT_ENGINE).ok();
            unsafe { std::env::set_var(ENV_STT_ENGINE, value) };
            Self { previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.as_deref() {
                Some(value) => unsafe { std::env::set_var(ENV_STT_ENGINE, value) },
                None => unsafe { std::env::remove_var(ENV_STT_ENGINE) },
            }
        }
    }

    #[test]
    #[serial]
    fn selected_engine_defaults_to_platform_auto_policy() {
        let _guard = EnvGuard::unset();
        let expected = if apple_stt::is_runtime_available() && apple_stt::is_bridge_resolvable() {
            SttEngine::Apple
        } else {
            SttEngine::Candle
        };
        assert_eq!(selected_engine(), expected);
    }

    #[test]
    #[serial]
    fn selected_engine_respects_explicit_overrides() {
        let _guard = EnvGuard::set("candle");
        assert_eq!(selected_engine(), SttEngine::Candle);

        unsafe { std::env::set_var(ENV_STT_ENGINE, "onnx") };
        assert_eq!(selected_engine(), SttEngine::Onnx);

        unsafe { std::env::set_var(ENV_STT_ENGINE, "apple") };
        assert_eq!(selected_engine(), SttEngine::Apple);
    }

    #[test]
    #[serial]
    fn selected_engine_auto_alias_uses_platform_default() {
        let _guard = EnvGuard::set("auto");
        assert_eq!(selected_engine(), default_engine());
    }
}
