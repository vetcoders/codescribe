//! Global Whisper engine singleton with embedded-first model provisioning.
//!
//! The canonical product path is an embedded Whisper payload built into the
//! binary. Runtime lookup remains as a fallback for explicit no-embed builds,
//! developer overrides, and recovery when the payload is unavailable.
//!
//! ## Idle unload
//!
//! The Whisper model lives on the GPU (Metal) and is by far the largest single
//! memory consumer (~3 GB resident). Keeping it loaded across long idle periods
//! wastes that memory, so the engine is held in a *resettable* slot: after a
//! configurable idle period with no transcription a background reaper drops it,
//! returning the GPU/host memory to the system, and the next call transparently
//! reloads it. Set `CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS=0` to disable.

// This entire module is a public API for library consumers

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tracing::{info, warn};

use crate::config::models::resolve_runtime_whisper_model_path;
use crate::config::{Config, UserSettings};
use crate::pipeline::contracts::{FileTranscriptionOptions, RawTranscript, TranscriptionVerdict};
use crate::pipeline::stream_postprocess::whisper_initial_prompt;

use super::engine::LocalWhisperEngine;
use super::params::DecodingParams;

/// Default model name (for dev/fallback mode)
pub use crate::config::models::DEFAULT_MODEL;

/// Default idle period after which the Whisper engine is unloaded to free GPU
/// memory. Disabled by default (0): each idle-unload→reload recreates the Metal
/// device (`Device::new_metal`), leaking IOAccelerator Mach ports + dispatch
/// threads per cycle and forcing a ~20-30s cold reload after the idle gap.
/// Keeping the engine resident (~3GB GPU floor) matches the old always-warm
/// daemon. Re-enable per machine via `CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS=<secs>`.
const DEFAULT_IDLE_UNLOAD_SECS: u64 = 0;

/// How often the reaper wakes to check for idleness.
const REAPER_TICK: Duration = Duration::from_secs(30);

/// Resettable engine slot: `None` when unloaded, plus the last-use timestamp the
/// reaper consults. A single `Mutex` serializes loads, transcriptions, and
/// unloads — exactly as the previous `Mutex<LocalWhisperEngine>` did.
struct WhisperSlot {
    engine: Option<LocalWhisperEngine>,
    last_used: Instant,
}

static SLOT: OnceLock<Mutex<WhisperSlot>> = OnceLock::new();

/// Runtime model path used only when embedded provisioning is unavailable.
static MODEL_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Guard so the idle reaper thread is spawned at most once.
static REAPER_STARTED: OnceLock<()> = OnceLock::new();

fn slot() -> &'static Mutex<WhisperSlot> {
    SLOT.get_or_init(|| {
        Mutex::new(WhisperSlot {
            engine: None,
            last_used: Instant::now(),
        })
    })
}

/// Resolve the configured idle-unload period, or `None` when disabled (0).
fn idle_unload_after() -> Option<Duration> {
    let secs = std::env::var("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_UNLOAD_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Resolve the model path for runtime Whisper fallback loading.
fn resolve_model_path_fallback() -> Result<PathBuf> {
    let local_model = configured_local_model();
    let resolved = resolve_runtime_whisper_model_path(Some(local_model.as_str()))?;
    info!(
        "Using runtime Whisper fallback model: {}",
        resolved.display()
    );
    Ok(resolved)
}

fn configured_local_model() -> String {
    std::env::var("LOCAL_MODEL")
        .ok()
        .and_then(non_empty)
        .or_else(|| UserSettings::load().local_model.and_then(non_empty))
        .or_else(|| {
            Config::parse_env_file(&Config::env_path())
                .ok()
                .and_then(|vars| vars.get("LOCAL_MODEL").cloned())
                .and_then(non_empty)
        })
        .unwrap_or_else(|| DEFAULT_MODEL.to_string())
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Get the resolved model path used by runtime Whisper fallback loading.
pub fn get_model_path() -> Result<&'static PathBuf> {
    if let Some(path) = MODEL_PATH.get() {
        return Ok(path);
    }

    let path = resolve_model_path_fallback()?;
    let _ = MODEL_PATH.set(path.clone());

    MODEL_PATH
        .get()
        .ok_or_else(|| anyhow!("Failed to store model path"))
}

/// Build a fresh engine, embedded-first with a runtime-path fallback.
fn load_engine() -> Result<LocalWhisperEngine> {
    // 1. Primary shipped path: embedded Whisper payload.
    if let Some(embedded) = super::embedded::get_embedded_data() {
        let engine = LocalWhisperEngine::from_embedded(&embedded)
            .context("Failed to initialize from embedded model")?;
        info!("Whisper engine loaded from embedded model (zero I/O)");
        return Ok(engine);
    }

    // 2. Fallback path: resolve Whisper model at runtime.
    let path = get_model_path()?;
    let engine = LocalWhisperEngine::new_with_params(path, DecodingParams::default())
        .context("Failed to initialize Whisper engine from path")?;
    info!("Whisper engine loaded from path: {}", path.display());
    Ok(engine)
}

/// Spawn the idle reaper once (only when idle-unload is enabled).
fn ensure_reaper() {
    if idle_unload_after().is_none() {
        return;
    }
    REAPER_STARTED.get_or_init(|| {
        let spawned = std::thread::Builder::new()
            .name("whisper-idle-reaper".into())
            .spawn(reaper_loop);
        if let Err(e) = spawned {
            warn!("Failed to spawn Whisper idle reaper: {e}");
        }
    });
}

/// Background loop: drops the engine after it has been idle long enough.
fn reaper_loop() {
    loop {
        std::thread::sleep(REAPER_TICK);
        let Some(threshold) = idle_unload_after() else {
            continue;
        };
        let mut guard = match slot().lock() {
            Ok(g) => g,
            Err(_) => continue,
        };
        if guard.engine.is_some() && guard.last_used.elapsed() >= threshold {
            // Drop the engine (and its Metal device) while holding the lock so
            // no transcription can be mid-flight, then return freed pages.
            guard.engine = None;
            drop(guard);
            info!(
                "Whisper engine unloaded after {}s idle; releasing GPU/host memory",
                threshold.as_secs()
            );
            crate::memory::release_freed_heap();
        }
    }
}

/// Run `f` with the engine, loading it on demand and refreshing the idle clock.
fn with_engine<R>(f: impl FnOnce(&mut LocalWhisperEngine) -> Result<R>) -> Result<R> {
    let mut guard = slot()
        .lock()
        .map_err(|e| anyhow!("Failed to lock engine: {}", e))?;
    if guard.engine.is_none() {
        guard.engine = Some(load_engine()?);
        ensure_reaper();
    }
    guard.last_used = Instant::now();
    let engine = guard
        .engine
        .as_mut()
        .ok_or_else(|| anyhow!("Engine not initialized"))?;
    f(engine)
}

fn with_engine_initial_prompt<R>(
    initial_prompt: Option<String>,
    f: impl FnOnce(&mut LocalWhisperEngine) -> Result<R>,
) -> Result<R> {
    with_engine(|engine| {
        let previous = engine.decoding_params.initial_prompt.clone();
        engine.decoding_params.initial_prompt = initial_prompt;
        let result = f(engine);
        engine.decoding_params.initial_prompt = previous;
        result
    })
}

fn file_transcription_initial_prompt() -> Option<String> {
    whisper_initial_prompt()
}

/// Like [`with_engine`] but never blocks: if the engine is busy, return an error
/// instead of waiting. Used by best-effort correction passes.
fn try_with_engine<R>(f: impl FnOnce(&mut LocalWhisperEngine) -> Result<R>) -> Result<R> {
    let mut guard = slot()
        .try_lock()
        .map_err(|_| anyhow!("Whisper engine busy, skipping correction"))?;
    if guard.engine.is_none() {
        guard.engine = Some(load_engine()?);
        ensure_reaper();
    }
    guard.last_used = Instant::now();
    let engine = guard
        .engine
        .as_mut()
        .ok_or_else(|| anyhow!("Engine not initialized"))?;
    f(engine)
}

/// Initialize the global engine (call once at startup).
///
/// Embedded Whisper is the product-default truth. Runtime path resolution is a
/// deliberate fallback for no-embed builds and local recovery. Idempotent: a
/// no-op if the engine is already loaded.
pub fn init() -> Result<()> {
    with_engine(|_| Ok(()))
}

/// Check if the engine is currently loaded.
///
/// Note: with idle-unload enabled this can become `false` again after a period
/// of inactivity; the next transcription call reloads transparently.
pub fn is_initialized() -> bool {
    SLOT.get()
        .and_then(|m| m.lock().ok().map(|g| g.engine.is_some()))
        .unwrap_or(false)
}

/// Transcribe audio samples using the global engine
pub fn transcribe(samples: &[f32], sample_rate: u32, language: Option<&str>) -> Result<String> {
    Ok(transcribe_with_segments(samples, sample_rate, language)?.text)
}

/// Transcribe audio samples with segment-level timestamps.
pub fn transcribe_with_segments(
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<RawTranscript> {
    with_engine(|engine| {
        engine.transcribe_long_with_language_segments(samples, sample_rate, language)
    })
}

/// Transcribe audio samples with a per-call Whisper initial prompt.
pub fn transcribe_with_segments_with_initial_prompt(
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
    initial_prompt: Option<String>,
) -> Result<RawTranscript> {
    with_engine_initial_prompt(initial_prompt, |engine| {
        engine.transcribe_long_with_language_segments(samples, sample_rate, language)
    })
}

/// Transcribe with streaming callback
pub fn transcribe_streaming<'a>(
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
    callback: Option<super::engine::ChunkCallback<'a>>,
) -> Result<String> {
    with_engine(|engine| engine.transcribe_long_streaming(samples, sample_rate, language, callback))
}

/// Transcribe a file with full structured verdict (VAD stats, confidence, provenance).
pub fn transcribe_file_verdict(
    path: &std::path::Path,
    language: Option<&str>,
    options: FileTranscriptionOptions,
) -> Result<TranscriptionVerdict> {
    with_engine_initial_prompt(file_transcription_initial_prompt(), |engine| {
        engine.transcribe_file_with_language(path, language, options)
    })
}

/// Detect language from audio samples
pub fn detect_language(samples: &[f32], sample_rate: u32) -> Result<String> {
    with_engine(|engine| engine.detect_language(samples, sample_rate))
}

/// Transcribe with a non-blocking engine acquisition (best-effort correction).
///
/// Returns an error instead of waiting if the engine is busy with another
/// transcription.
pub fn try_transcribe_with_segments(
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<RawTranscript> {
    try_with_engine(|engine| {
        engine.transcribe_long_with_language_segments(samples, sample_rate, language)
    })
}

/// Transcribe a single (already-windowed) chunk, blocking on the engine.
pub fn transcribe_chunk(
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<String> {
    with_engine(|engine| engine.transcribe_with_language(samples, sample_rate, language))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;

    struct EnvRestore {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvRestore {
        fn capture(key: &'static str) -> Self {
            Self {
                key,
                previous: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    #[serial]
    fn file_transcription_initial_prompt_defaults_off() {
        let _data_dir = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _env_path = EnvRestore::capture("CODESCRIBE_ENV_PATH");
        let _prompt_enabled = EnvRestore::capture(
            crate::pipeline::stream_postprocess::STT_INITIAL_PROMPT_ENABLED_ENV,
        );
        let temp_dir = tempfile::tempdir().expect("temp data dir");

        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp_dir.path());
            std::env::remove_var("CODESCRIBE_ENV_PATH");
            std::env::remove_var(
                crate::pipeline::stream_postprocess::STT_INITIAL_PROMPT_ENABLED_ENV,
            );
        }

        assert_eq!(file_transcription_initial_prompt(), None);
    }

    #[test]
    #[serial]
    fn file_transcription_initial_prompt_is_opt_in() {
        let _data_dir = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _env_path = EnvRestore::capture("CODESCRIBE_ENV_PATH");
        let _prompt_enabled = EnvRestore::capture(
            crate::pipeline::stream_postprocess::STT_INITIAL_PROMPT_ENABLED_ENV,
        );
        let temp_dir = tempfile::tempdir().expect("temp data dir");

        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp_dir.path());
            std::env::remove_var("CODESCRIBE_ENV_PATH");
            std::env::set_var(
                crate::pipeline::stream_postprocess::STT_INITIAL_PROMPT_ENABLED_ENV,
                "1",
            );
        }

        let prompt = file_transcription_initial_prompt().expect("opt-in prompt should be built");
        assert!(prompt.contains("Loctree"));
    }

    #[test]
    fn idle_unload_disabled_when_zero() {
        // SAFETY: single-threaded test mutating a process env var it owns.
        unsafe { std::env::set_var("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS", "0") };
        assert!(idle_unload_after().is_none());
        unsafe { std::env::set_var("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS", "120") };
        assert_eq!(idle_unload_after(), Some(Duration::from_secs(120)));
        unsafe { std::env::remove_var("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS") };
        // DEFAULT_IDLE_UNLOAD_SECS is now 0 (idle-unload disabled by default),
        // so with no override the reaper is off.
        assert!(idle_unload_after().is_none());
    }

    #[test]
    #[serial]
    fn configured_local_model_prefers_env_then_settings_then_env_file() {
        let _data_dir = EnvRestore::capture("CODESCRIBE_DATA_DIR");
        let _local_model = EnvRestore::capture("LOCAL_MODEL");
        let temp_dir = tempfile::tempdir().expect("temp data dir");

        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", temp_dir.path());
            std::env::remove_var("LOCAL_MODEL");
        }

        let env_path = Config::env_path();
        std::fs::create_dir_all(env_path.parent().expect("env parent")).expect("env dir");
        std::fs::write(&env_path, "LOCAL_MODEL=env-file-model\n").expect("env file");

        assert_eq!(configured_local_model(), "env-file-model");

        let mut settings = UserSettings::load();
        settings.set_string("LOCAL_MODEL", "settings-model");
        assert_eq!(configured_local_model(), "settings-model");

        unsafe { std::env::set_var("LOCAL_MODEL", "runtime-model") };
        assert_eq!(configured_local_model(), "runtime-model");
    }

    #[test]
    #[serial]
    fn test_model_path_resolution_and_real_whisper_noop_load() {
        let path = match resolve_model_path_fallback() {
            Ok(path) => path,
            Err(err) => {
                println!("No model found (expected in CI): {err:?}");
                return;
            }
        };

        assert!(path.join("tokenizer.json").exists());
        println!("Found model at: {}", path.display());

        // This is the real contract we care about in core tests:
        // if the runtime can resolve a model, Whisper must actually load and
        // survive a no-op transcription path without mocking the engine.
        let text = transcribe(&[], 16_000, Some("pl")).expect("Whisper no-op load should work");
        assert!(
            text.is_empty(),
            "empty input should stay empty after no-op load"
        );
    }
}
