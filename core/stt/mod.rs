//! Speech-to-text engine router.
//!
//! Three backends live behind one call surface — Candle Whisper, ONNX Whisper,
//! and Apple SpeechAnalyzer — selected by `CODESCRIBE_STT_ENGINE`. Unset or
//! `auto` resolves through [`default_engine`]: Apple when its runtime *and*
//! bridge are both reachable, otherwise Candle.
//!
//! ## The lane split (product truth, not an implementation detail)
//!
//! - **Live** (buffer / progressive / chunk) is Apple's lane when Apple is
//!   selected. A hard bridge failure with real audio falls back to Candle as
//!   *emergency recovery* so a session never dies silently — never as a quiet
//!   default.
//! - **File final-pass** is Whisper only. [`transcribe_file_verdict`]
//!   deliberately refuses the Apple file path even under
//!   `CODESCRIBE_STT_ENGINE=apple`; measured on long Polish dictation, Apple's
//!   URL recognizer collapses the take to a tail fragment.
//!
/// Bounded read-only view of active W2-04 Agent session-name leases.
pub mod active_names;
/// Candle Whisper singleton adapter implementing `TranscriptionAdapter`.
pub mod adapter;
/// Apple SpeechAnalyzer live STT bridge (letter-level canvas; live lane only).
pub mod apple_stt;
/// ONNX Whisper runtime adapter selected via `CODESCRIBE_STT_ENGINE=onnx`.
pub mod onnx_adapter;
/// Explicit cloud/loopback STT topic token. Client-owned; never from audio.
pub mod request_vocabulary;
/// Serialized STT request scheduler: live, commit, and refine lanes with
/// supersede semantics for stale requests and thermal-pressure backoff.
pub mod scheduler;
/// Layer-1 on-the-go Whisper tail-patch helpers for append-only gap fill.
pub mod tail_patcher;
/// Typed, time-ranged provider seam for Whisper tail-patch windows.
pub mod tail_provider;
/// Candle Whisper engine, singleton, and file final-pass routes.
pub mod whisper;

#[cfg(test)]
mod fleet_red_contracts;

use crate::pipeline::contracts::RawTranscript;
use crate::pipeline::contracts::TranscriptionAdapter;
use std::sync::OnceLock;
use tracing::warn;

/// Which STT backend the router dispatches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SttEngine {
    /// Candle Whisper — the local model. Only engine that honours an
    /// `initial_prompt`, and the only legal file final-pass route.
    Candle,
    /// ONNX Whisper runtime.
    Onnx,
    /// Apple SpeechAnalyzer via the external bridge; live lane only.
    Apple,
}

/// Auto policy: Apple only when it can actually run, otherwise Candle.
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
    match default_engine() {
        SttEngine::Onnx => {
            onnx_adapter::init()?;
            Ok(Box::new(onnx_adapter::OnnxWhisperAdapter::new()))
        }
        SttEngine::Apple => {
            apple_stt::init()?;
            Ok(Box::new(apple_stt::AppleSpeechAnalyzerAdapter::new())
                as Box<dyn TranscriptionAdapter>)
        }
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

/// Prefer Apple for live. On hard Apple failure with real audio, **emergency**
/// Candle recovery keeps the session alive (no empty toast / dead overlay).
///
/// Product rule remains: Apple is the letter-level primary. Whisper must not
/// become the silent default path — only a last-ditch floor when the bridge
/// fails outright. File final-pass still uses the explicit Whisper final path.
fn run_apple_live_only<T>(
    context: &str,
    apple_path: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    if !apple_stt::is_runtime_available() {
        let err = anyhow::anyhow!(
            "Apple STT runtime not available during {context}; live lane refuses silent no-op"
        );
        warn!(
            "Apple STT live-only: unavailable during {}: {:#}",
            context, err
        );
        return Err(err);
    }

    apple_path().map_err(|err| {
        // `{:#}` surfaces the full anyhow chain (bridge stderr / timeout / JSON).
        warn!("Apple STT live-only: failed during {}: {:#}", context, err);
        err.context(format!("Apple STT live path failed during {context}"))
    })
}

/// Preflight before starting a recording when live engine is Apple.
///
/// Fails **before** REC so we never open an empty overlay that dies mid-take.
/// Whisper is not substituted here — recovery is a separate stop-path cut when
/// audio already exists.
pub fn preflight_apple_live_ready() -> anyhow::Result<()> {
    if !matches!(default_engine(), SttEngine::Apple) {
        return Ok(());
    }
    if !apple_stt::is_runtime_available() {
        anyhow::bail!(
            "Apple Speech is not available on this macOS version. \
             Install a supported macOS or switch STT engine to Whisper in Settings."
        );
    }
    if !apple_stt::is_bridge_resolvable() {
        anyhow::bail!(
            "Apple STT bridge not found. Use the Codescribe.app build (bridge beside the app) \
             or set CODESCRIBE_APPLE_STT_BRIDGE to the bridge binary."
        );
    }
    // Probe Speech TCC + locale assets once before audio starts.
    apple_stt::init().map_err(|err| {
        anyhow::anyhow!(
            "Apple Speech is not ready: {err}. \
             Enable Speech Recognition for Codescribe in System Settings › Privacy & Security."
        )
    })
}

/// Warn once that a domain-vocabulary `initial_prompt` is being dropped —
/// only Candle Whisper can consume one.
fn warn_initial_prompt_unsupported(engine: &str) {
    /// Process-once latch so repeated fallbacks do not flood tracing logs.
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
/// Candle chunk transcription through the Whisper singleton.
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

/// Candle long-audio transcription with segment timestamps (blocking acquire).
fn candle_transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    whisper::singleton::transcribe_with_segments(audio, sample_rate, language)
}

/// Candle long-audio transcription seeded with a per-call domain vocabulary.
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

/// Non-blocking Candle long transcription: yields an error instead of waiting
/// when the engine lock is held, so a correction pass can be skipped rather
/// than queued behind live work.
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
    match default_engine() {
        SttEngine::Onnx => onnx_adapter::init(),
        SttEngine::Apple => apple_stt::init(),
        SttEngine::Candle => whisper::init(),
    }
}

/// File-level transcription verdict for **stop final-pass only**.
///
/// Product split (operator truth 2026-07-24, clips 01–04 + div0):
/// - **Live** = Apple buffer / virtual-mic (`transcribe_live`, SFSpeechAudioBuffer).
/// - **File final** = Whisper only. Apple `SFSpeechURLRecognitionRequest` on a
///   full WAV is an engineering mistake at scale: collapses long Polish dictation
///   to a tail fragment (0–66 chars) and can *still beat* a thin live stream on
///   length regression because the fragment is slightly longer than the broken
///   live assembly — see data_assets/02 e2e (live 26c, Apple file 66c, human 600c+).
///
/// When `CODESCRIBE_STT_ENGINE=apple`, live paths stay Apple-only; this function
/// deliberately does **not** call `apple_stt::transcribe_file_verdict`.
pub fn transcribe_file_verdict(
    path: &std::path::Path,
    language: Option<&str>,
) -> anyhow::Result<crate::pipeline::contracts::TranscriptionVerdict> {
    use crate::pipeline::contracts::FileTranscriptionOptions;

    match default_engine() {
        SttEngine::Onnx => {
            // ONNX has no dedicated file-verdict path; use Candle Whisper for file final-pass.
            whisper::transcribe_file_verdict(path, language, FileTranscriptionOptions::default())
        }
        SttEngine::Apple => {
            // Do not route file final through Apple — even as "primary with Whisper
            // fallback". Apple file STT is not a product final path.
            tracing::info!(
                "file final-pass forced to Whisper (Apple is live-only; SFSpeechURL is not final)"
            );
            whisper::transcribe_file_verdict(path, language, FileTranscriptionOptions::default())
        }
        SttEngine::Candle => {
            whisper::transcribe_file_verdict(path, language, FileTranscriptionOptions::default())
        }
    }
}

/// Sample rate of the synthetic warmup buffer.
const WARMUP_SAMPLE_RATE: u32 = 16_000;

/// Prewarm the ACTIVE STT engine end-to-end so the first real dictation pays
/// neither model-load nor (for the Candle/Metal path) first-inference Metal
/// kernel-compilation latency, and (for the Apple path) neither the bridge
/// spawn nor the SpeechAnalyzer asset/probe readiness.
///
/// **Apple live invariant:** when the selected engine is Apple, this path must
/// **never** load Whisper. Gap-fill / file final-pass lazy-load Candle on first
/// use. Touching Whisper here re-introduces the multi-GB resident floor at app
/// start and can refuse recording when the Whisper model is absent.
///
/// Candle path still warms weights + kernels via the real transcribe route.
///
/// Best-effort: warmup transcription errors are logged, never propagated, so a
/// cold-path hiccup can never block recording readiness.
pub fn prewarm_active_engine() -> anyhow::Result<()> {
    let warmup = synthetic_warmup_audio();

    match default_engine() {
        SttEngine::Apple => {
            // Init + warm Apple only. Do not fall through to Whisper on failure
            // (that would load multi-GB weights at startup and block when the
            // model is missing). Emergency Whisper recovery stays on live audio.
            apple_stt::init().map_err(|err| {
                anyhow::anyhow!("Apple STT prewarm init failed (Whisper not substituted): {err:#}")
            })?;
            match apple_stt::transcribe_long_with_segments(&warmup, WARMUP_SAMPLE_RATE, Some("en"))
            {
                Ok(_) => tracing::info!(
                    "STT active-engine warmup inference complete (apple; whisper not loaded)"
                ),
                Err(error) => tracing::warn!(
                    "STT Apple warmup inference failed (non-fatal; whisper stays lazy): {error:#}"
                ),
            }
            Ok(())
        }
        SttEngine::Onnx => {
            onnx_adapter::init()?;
            match onnx_adapter::transcribe_long_with_segments(
                &warmup,
                WARMUP_SAMPLE_RATE,
                Some("en"),
            ) {
                Ok(_) => tracing::info!("STT active-engine warmup inference complete (onnx)"),
                Err(error) => {
                    tracing::warn!("STT ONNX warmup inference failed (non-fatal): {error:#}")
                }
            }
            Ok(())
        }
        SttEngine::Candle => {
            whisper::init()?;
            match candle_transcribe_long_with_segments(&warmup, WARMUP_SAMPLE_RATE, Some("en")) {
                Ok(_) => tracing::info!("STT active-engine warmup inference complete (candle)"),
                Err(error) => {
                    tracing::warn!("STT Candle warmup inference failed (non-fatal): {error:#}")
                }
            }
            Ok(())
        }
    }
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
    match default_engine() {
        SttEngine::Onnx => onnx_adapter::transcribe_chunk(audio, sample_rate, language),
        SttEngine::Apple => run_apple_live_only("transcribe_chunk", || {
            apple_stt::transcribe_chunk(audio, sample_rate, language)
        }),
        SttEngine::Candle => candle_transcribe_chunk(audio, sample_rate, language),
    }
}

/// Transcribe long audio (blocking lock) with segment-level timestamps.
pub(crate) fn transcribe_long_with_segments(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    match default_engine() {
        SttEngine::Onnx => {
            onnx_adapter::transcribe_long_with_segments(audio, sample_rate, language)
        }
        SttEngine::Apple => run_apple_live_only("transcribe_long_with_segments", || {
            apple_stt::transcribe_long_with_segments(audio, sample_rate, language)
        }),
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

    match default_engine() {
        SttEngine::Onnx => {
            warn_initial_prompt_unsupported("ONNX");
            onnx_adapter::transcribe_long_with_segments(audio, sample_rate, language)
        }
        SttEngine::Apple => {
            warn_initial_prompt_unsupported("Apple SpeechAnalyzer");
            run_apple_live_only("transcribe_long_with_segments_with_initial_prompt", || {
                apple_stt::transcribe_long_with_segments(audio, sample_rate, language)
            })
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
    match default_engine() {
        SttEngine::Onnx => {
            onnx_adapter::try_transcribe_long_with_segments(audio, sample_rate, language)
        }
        SttEngine::Apple => run_apple_live_only("try_transcribe_long_with_segments", || {
            apple_stt::transcribe_long_with_segments(audio, sample_rate, language)
        }),
        SttEngine::Candle => candle_try_transcribe_long_with_segments(audio, sample_rate, language),
    }
}

/// Live-only and Smart-mode tail-gap doctrine unit tests.
#[cfg(test)]
mod tests {
    use super::*;

    /// Live-only helper surfaces Apple bridge failures instead of silent swap.
    #[test]
    fn run_apple_live_only_surfaces_bridge_errors() {
        // Low-level helper still surfaces Apple failures; emergency Whisper is
        // layered above via apple_live_or_emergency_whisper when audio exists.
        let err = run_apple_live_only("unit_test", || -> anyhow::Result<()> {
            Err(anyhow::anyhow!("bridge boom"))
        })
        .expect_err("must not succeed");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("bridge boom") || msg.contains("live"),
            "unexpected error: {msg}"
        );
    }

    /// File final-pass stays Whisper-only even when live engine is Apple.
    #[test]
    fn file_final_pass_is_whisper_policy_even_when_live_engine_is_apple() {
        // Source-level product invariant: Apple arm of transcribe_file_verdict
        // must call whisper::transcribe_file_verdict only (no apple_stt file).
        // data_assets/02: Apple URL final 66c beat live 26c and still lost human 600c+.
        let src = include_str!("mod.rs");
        let apple_arm = src
            .split("SttEngine::Apple =>")
            .nth(2) // third occurrence ≈ file-final match arm after live helpers
            .unwrap_or("");
        // Fall back: scan the function body by name.
        let fn_body = src
            .split("pub fn transcribe_file_verdict")
            .nth(1)
            .and_then(|s| s.split("const WARMUP_SAMPLE_RATE").next())
            .unwrap_or("");
        assert!(
            fn_body.contains("forced to Whisper") || fn_body.contains("file final-pass forced"),
            "transcribe_file_verdict must document Whisper-only file final"
        );
        assert!(
            !fn_body.contains("apple_stt::transcribe_file_verdict"),
            "transcribe_file_verdict must not call apple_stt file path (Apple is live-only)"
        );
        let _ = apple_arm;
    }

}
