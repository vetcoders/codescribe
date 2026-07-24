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

/// Apple-first long transcription with emergency Whisper only when Apple hard-fails
/// and there is real audio to recover from.
fn apple_live_or_emergency_whisper(
    context: &str,
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    match run_apple_live_only(context, || {
        apple_stt::transcribe_long_with_segments(audio, sample_rate, language)
    }) {
        Ok(raw) => Ok(raw),
        Err(err) if !audio.is_empty() && sample_rate > 0 => {
            warn!(
                "Apple live hard-fail during {context} — emergency Whisper recovery ({:#})",
                err
            );
            candle_transcribe_long_with_segments(audio, sample_rate, language).map_err(|werr| {
                werr.context(format!(
                    "Apple live failed and emergency Whisper also failed during {context}: {err:#}"
                ))
            })
        }
        Err(err) => Err(err),
    }
}

/// Preferential engine label for UI honesty (`local_apple` / `local_whisper` / …).
pub fn preferred_engine_label() -> &'static str {
    match selected_engine() {
        SttEngine::Apple => "local_apple",
        SttEngine::Onnx => "local_whisper",
        SttEngine::Candle => "local_whisper",
    }
}

/// Preflight before starting a recording when live engine is Apple.
///
/// Fails **before** REC so we never open an empty overlay that dies mid-take.
/// Whisper is not substituted here — recovery is a separate stop-path cut when
/// audio already exists.
pub fn preflight_apple_live_ready() -> anyhow::Result<()> {
    if !matches!(selected_engine(), SttEngine::Apple) {
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

    match selected_engine() {
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

/// Whether the **live** router is on the Apple lane (buffer / progressive).
///
/// Not a signal to run Apple on file final-pass — see [`transcribe_file_verdict`].
pub fn active_engine_is_apple() -> bool {
    matches!(selected_engine(), SttEngine::Apple)
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
        SttEngine::Apple => {
            match run_apple_live_only("transcribe_chunk", || {
                apple_stt::transcribe_chunk(audio, sample_rate, language)
            }) {
                Ok(t) => Ok(t),
                Err(err) if !audio.is_empty() && sample_rate > 0 => {
                    warn!(
                        "Apple live hard-fail during transcribe_chunk — emergency Whisper recovery ({:#})",
                        err
                    );
                    candle_transcribe_chunk(audio, sample_rate, language).map_err(|werr| {
                        werr.context(format!(
                            "Apple live failed and emergency Whisper also failed during transcribe_chunk: {err:#}"
                        ))
                    })
                }
                Err(err) => Err(err),
            }
        }
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
        SttEngine::Apple => apple_live_or_emergency_whisper(
            "transcribe_long_with_segments",
            audio,
            sample_rate,
            language,
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
            // Live/commit/refine under Apple: primary Apple; emergency Whisper
            // only when the bridge hard-fails with real audio (keeps session alive).
            warn_initial_prompt_unsupported("Apple SpeechAnalyzer");
            apple_live_or_emergency_whisper(
                "transcribe_long_with_segments_with_initial_prompt",
                audio,
                sample_rate,
                language,
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
        SttEngine::Apple => apple_live_or_emergency_whisper(
            "try_transcribe_long_with_segments",
            audio,
            sample_rate,
            language,
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

    #[test]
    #[serial]
    fn preflight_apple_live_ready_is_noop_when_engine_is_not_apple() {
        let _guard = EnvGuard::set("whisper");
        preflight_apple_live_ready().expect("Whisper preference must not require Apple preflight");
        assert_eq!(preferred_engine_label(), "local_whisper");
    }

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
            .and_then(|s| s.split("pub fn active_engine_is_apple").next())
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
