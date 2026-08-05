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

/// First sample index of the uncommitted tail, clamped into `0..=total_samples`.
///
/// `from_secs` is the end timestamp of the last **committed** utterance. A
/// non-positive boundary means nothing is committed yet (whole file); a
/// boundary past the recording yields `total_samples` (empty tail).
fn tail_gap_start_index(total_samples: usize, sample_rate: u32, from_secs: f32) -> usize {
    if sample_rate == 0 || !from_secs.is_finite() || from_secs <= 0.0 {
        return 0;
    }
    let offset = (from_secs as f64) * (sample_rate as f64);
    if offset >= total_samples as f64 {
        return total_samples;
    }
    (offset.round() as usize).min(total_samples)
}

/// Where a Smart-mode tail gap-fill is allowed to start — or whether it is
/// allowed at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TailGapBoundary {
    /// Transcribe the audio from this (positive) committed boundary onward.
    From(f32),
    /// No boundary, but the streaming canvas is EMPTY — gap-filling the whole
    /// session into nothing is still an append, so it is legal.
    WholeSessionBootstrap,
    /// No boundary and a non-empty canvas: any Whisper pass here would be a
    /// whole-file re-pass landing on committed text. Honest skip instead.
    Skip,
}

/// Resolve the tail gap-fill boundary from session telemetry — the guard that
/// keeps Smart mode out of full-file re-pass territory.
///
/// `committed_through_secs` is the end timestamp of the last committed
/// utterance; it is `None` (or non-positive/non-finite) whenever the session
/// produced no usable commit evidence — `no_commit_source`, `no_coverage`,
/// `empty`. Treating that as `0.0` and calling the tail primitive transcribes
/// the WHOLE file and appends it to the existing streaming text: exactly the
/// replacement/duplication the append-only overlay doctrine forbids outside
/// `FINAL_PASS_MODE=Always`.
pub fn resolve_tail_gap_boundary(
    committed_through_secs: Option<f32>,
    streaming_is_empty: bool,
) -> TailGapBoundary {
    match committed_through_secs {
        Some(secs) if secs.is_finite() && secs > 0.0 => TailGapBoundary::From(secs),
        _ if streaming_is_empty => TailGapBoundary::WholeSessionBootstrap,
        _ => TailGapBoundary::Skip,
    }
}

/// Transcribe **only the uncommitted tail** of a recording — the Smart-mode
/// per-utterance gap-fill primitive.
///
/// Loads `path`, drops everything before `from_secs` (the end of the last
/// committed utterance) and runs [`whisper_tail_patch_transcribe`] on that
/// slice alone. The result is meant to be **appended** to committed streaming
/// text; committed text is immutable (append-only overlay doctrine).
///
/// Boundaries:
/// - `from_secs <= 0.0` → the whole file (nothing committed yet).
/// - `from_secs` at/after the recording end → `Ok(RawTranscript::default())`
///   without touching Whisper.
///
/// This must never be handed the full file when a positive boundary exists —
/// a full-file re-pass is `FINAL_PASS_MODE=Always` territory only.
pub fn whisper_tail_gap_transcribe_file(
    path: &std::path::Path,
    from_secs: f32,
    language: Option<&str>,
) -> anyhow::Result<RawTranscript> {
    let (samples, sample_rate) = crate::audio::load_audio_file(path)?;
    let start = tail_gap_start_index(samples.len(), sample_rate, from_secs);
    let tail = &samples[start..];
    if tail.is_empty() {
        tracing::debug!(
            "tail-gap transcribe: nothing uncommitted after {:.3}s in {} ({} samples @ {} Hz)",
            from_secs,
            path.display(),
            samples.len(),
            sample_rate
        );
        return Ok(RawTranscript::default());
    }
    whisper_tail_patch_transcribe(tail, sample_rate, language)
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

    match selected_engine() {
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

    // ═══════════════════════════════════════════════════════
    // Smart-mode tail-gap primitive (append-only doctrine)
    // ═══════════════════════════════════════════════════════

    #[test]
    fn tail_gap_start_index_is_clamped_sample_math() {
        // Whole file: a non-positive boundary means "nothing committed yet".
        assert_eq!(tail_gap_start_index(32_000, 16_000, 0.0), 0);
        assert_eq!(tail_gap_start_index(32_000, 16_000, -3.5), 0);

        // Mid-file boundary: sample_rate × secs.
        assert_eq!(tail_gap_start_index(32_000, 16_000, 1.0), 16_000);
        assert_eq!(tail_gap_start_index(32_000, 16_000, 1.5), 24_000);
        assert_eq!(tail_gap_start_index(96_000, 48_000, 0.5), 24_000);

        // Beyond the audio: clamped to len (empty tail, never a panic).
        assert_eq!(tail_gap_start_index(32_000, 16_000, 9.0), 32_000);
        assert_eq!(tail_gap_start_index(0, 16_000, 1.0), 0);

        // Degenerate sample rate cannot produce a bogus offset.
        assert_eq!(tail_gap_start_index(32_000, 0, 1.0), 0);
    }

    #[test]
    fn whisper_tail_gap_transcribe_file_returns_empty_for_silent_tail() {
        // ~2s of digital silence @16k: VAD finds no speech, so the gap-fill
        // short-circuits before any Whisper model load.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("silence.wav");
        let samples = vec![0.0f32; 32_000];
        crate::tts::AudioPlayer::save_wav(&samples, 16_000, &path).expect("save_wav");

        let out = whisper_tail_gap_transcribe_file(&path, 0.5, Some("pl"))
            .expect("silent tail must not error");
        assert!(
            out.text.trim().is_empty(),
            "silent tail must yield empty transcript, got {:?}",
            out.text
        );
        assert!(
            out.segments.is_empty(),
            "silent tail must yield no segments"
        );
    }

    #[test]
    fn whisper_tail_gap_transcribe_file_beyond_duration_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("silence.wav");
        let samples = vec![0.0f32; 32_000];
        crate::tts::AudioPlayer::save_wav(&samples, 16_000, &path).expect("save_wav");

        // Boundary past the end of the recording: nothing uncommitted remains.
        let out = whisper_tail_gap_transcribe_file(&path, 30.0, Some("pl"))
            .expect("out-of-range boundary must not error");
        assert!(out.text.is_empty());
        assert!(out.segments.is_empty());
    }

    /// THE DOCTRINE MATRIX: a missing/zero commit boundary must never turn a
    /// Smart-mode gap-fill into a whole-file Whisper pass appended onto existing
    /// streaming text. Bootstrap (empty canvas) is the only legal whole-session
    /// gap-fill; everything else with no boundary is an honest skip.
    #[test]
    fn resolve_tail_gap_boundary_matrix() {
        // Positive boundary → tail from that boundary, in both canvas states.
        assert_eq!(
            resolve_tail_gap_boundary(Some(4.25), false),
            TailGapBoundary::From(4.25)
        );
        assert_eq!(
            resolve_tail_gap_boundary(Some(4.25), true),
            TailGapBoundary::From(4.25)
        );
        assert_eq!(
            resolve_tail_gap_boundary(Some(f32::MIN_POSITIVE), false),
            TailGapBoundary::From(f32::MIN_POSITIVE)
        );

        // No boundary + EMPTY canvas → whole-session bootstrap is a legal append.
        assert_eq!(
            resolve_tail_gap_boundary(None, true),
            TailGapBoundary::WholeSessionBootstrap
        );
        assert_eq!(
            resolve_tail_gap_boundary(Some(0.0), true),
            TailGapBoundary::WholeSessionBootstrap
        );
        assert_eq!(
            resolve_tail_gap_boundary(Some(-1.0), true),
            TailGapBoundary::WholeSessionBootstrap
        );
        assert_eq!(
            resolve_tail_gap_boundary(Some(f32::NAN), true),
            TailGapBoundary::WholeSessionBootstrap
        );
        assert_eq!(
            resolve_tail_gap_boundary(Some(f32::INFINITY), true),
            TailGapBoundary::WholeSessionBootstrap
        );

        // No boundary + NON-EMPTY canvas → skip. A whole-file re-pass appended
        // onto committed text is forbidden outside Always mode.
        assert_eq!(
            resolve_tail_gap_boundary(None, false),
            TailGapBoundary::Skip
        );
        assert_eq!(
            resolve_tail_gap_boundary(Some(0.0), false),
            TailGapBoundary::Skip
        );
        assert_eq!(
            resolve_tail_gap_boundary(Some(-0.001), false),
            TailGapBoundary::Skip
        );
        assert_eq!(
            resolve_tail_gap_boundary(Some(f32::NAN), false),
            TailGapBoundary::Skip
        );
        assert_eq!(
            resolve_tail_gap_boundary(Some(f32::NEG_INFINITY), false),
            TailGapBoundary::Skip
        );
    }
}
