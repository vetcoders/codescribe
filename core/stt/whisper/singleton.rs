//! Global Whisper engine singleton with embedded-first model provisioning.
//!
//! The canonical product path is an embedded Whisper payload built into the
//! binary. Runtime lookup remains as a fallback for explicit no-embed builds,
//! developer overrides, and recovery when the payload is unavailable.
//!
//! ## Idle unload
//!
//! The Whisper model lives on the GPU (Metal) and is by far the largest single
//! memory consumer (~3–7 GB resident after a good pass). Keeping it loaded
//! across long idle periods wastes that memory, so the engine is held in a
//! *resettable* slot: after a configurable idle period with no transcription a
//! background reaper drops the **weights** (the `LocalWhisperEngine`), and the
//! next call transparently reloads them.
//!
//! The Candle Metal `Device` is **not** recreated on reload — it is process-
//! cached in `engine::process_device` so unload→reload does not leak
//! IOAccelerator Mach ports / dispatch threads (the reason idle-unload was
//! previously disabled with `DEFAULT_IDLE_UNLOAD_SECS = 0`).
//!
//! Dropping the weights alone is not enough: their `MTLBuffer`s return to the
//! `MetalDevice` free-buffer pool, which candle only prunes during the next
//! inference. The reaper therefore forces that prune right after the drop
//! (`memory::reclaim_metal_buffer_pool`) so RSS actually falls while idle.
//!
//! Default TTL is one minute. Set `CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS=0` to
//! explicitly keep weights resident for the whole process life.

// This entire module is a public API for library consumers

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tracing::{info, warn};

use crate::config::models::resolve_runtime_whisper_model_path;
use crate::config::{Config, UserSettings};
use crate::pipeline::contracts::{FileTranscriptionOptions, RawTranscript, TranscriptionVerdict};

use super::engine::LocalWhisperEngine;
use super::params::DecodingParams;

/// Default model name (for dev/fallback mode)
pub use crate::config::models::DEFAULT_MODEL;

/// Default idle period after which Whisper **weights** are unloaded.
///
/// One minute bounds the operator-measured multi-GB resident floor while the
/// full fp16 payload keeps the next cold load free of q8 dequantization. An
/// explicit `0` override remains available for power users who choose
/// keep-warm.
/// Metal `Device` stays process-cached (see engine module), so reloads after
/// TTL reuse the same device.
const DEFAULT_IDLE_UNLOAD_SECS: u64 = 60;

/// How often the reaper wakes to check for idleness.
const REAPER_TICK: Duration = Duration::from_secs(30);

/// Resettable engine slot: `None` when unloaded, plus the last-use timestamp the
/// reaper consults. A single `Mutex` serializes loads, transcriptions, and
/// unloads — exactly as the previous `Mutex<LocalWhisperEngine>` did.
struct WhisperSlot {
    engine: Option<LocalWhisperEngine>,
    last_used: Instant,
}

/// The one process-wide engine slot. Lazily created by [`slot`]; `OnceLock`
/// guards creation of the `Mutex`, the `Mutex` guards the engine inside it.
static SLOT: OnceLock<Mutex<WhisperSlot>> = OnceLock::new();

/// Runtime model path used only when embedded provisioning is unavailable.
static MODEL_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Guard so the idle reaper thread is spawned at most once.
static REAPER_STARTED: OnceLock<()> = OnceLock::new();

/// Process-lifetime residency transition counters. These deliberately record
/// lifecycle only: no audio, transcript, or model-path content enters them.
static RESIDENCY_LOAD_COUNT: AtomicU64 = AtomicU64::new(0);
static RESIDENCY_UNLOAD_COUNT: AtomicU64 = AtomicU64::new(0);
static RESIDENCY_RECLAIM_COUNT: AtomicU64 = AtomicU64::new(0);

/// Test-only witness for callers that would initialize the heavyweight local
/// engine. It lets routing tests exercise the real selected-engine seam without
/// loading a model or inferring from source text.
#[cfg(test)]
static TEST_INIT_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
static TEST_LOAD_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Access the engine slot, creating it (unloaded) on first use.
fn slot() -> &'static Mutex<WhisperSlot> {
    SLOT.get_or_init(|| {
        Mutex::new(WhisperSlot {
            engine: None,
            last_used: Instant::now(),
        })
    })
}

/// The effective residency policy at this instant.
///
/// `effective_ttl_secs=0` is intentionally a meaningful, explicit keep-warm
/// value rather than an absent configuration. Every residency lifecycle log
/// emits both fields so an operator dotenv override cannot masquerade as the
/// shipped default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WhisperResidencyPolicy {
    effective_ttl_secs: u64,
    keep_warm: bool,
}

fn whisper_residency_policy() -> WhisperResidencyPolicy {
    let secs = std::env::var("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_UNLOAD_SECS);
    WhisperResidencyPolicy {
        effective_ttl_secs: secs,
        keep_warm: secs == 0,
    }
}

/// Resolve the configured idle-unload period, or `None` for keep-warm.
fn idle_unload_after() -> Option<Duration> {
    let policy = whisper_residency_policy();
    (!policy.keep_warm).then(|| Duration::from_secs(policy.effective_ttl_secs))
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

/// Resolve which Whisper model the runtime fallback should look for.
///
/// Precedence, first non-empty wins: `LOCAL_MODEL` in the process environment,
/// then the persisted [`UserSettings`] value, then `LOCAL_MODEL` in the on-disk
/// env file, then [`DEFAULT_MODEL`]. Only consulted when embedded Whisper is
/// unavailable.
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

/// Trim a configured value and treat blank as absent, so an empty `LOCAL_MODEL=`
/// falls through to the next precedence tier instead of resolving to `""`.
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
    #[cfg(test)]
    TEST_LOAD_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

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

/// Emit the load transition without leaking transcription content.
fn record_residency_load(model_load_ms: u64) {
    let policy = whisper_residency_policy();
    let load_count = RESIDENCY_LOAD_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    info!(
        event = "whisper_residency_load",
        load_count,
        model_load_ms,
        effective_ttl_secs = policy.effective_ttl_secs,
        keep_warm = policy.keep_warm,
        "Whisper residency load complete"
    );
}

/// Spawn the idle reaper once (only when idle-unload is enabled).
fn ensure_reaper() {
    let policy = whisper_residency_policy();
    if policy.keep_warm {
        info!(
            event = "whisper_residency_policy",
            effective_ttl_secs = policy.effective_ttl_secs,
            keep_warm = true,
            "Whisper residency keep-warm selected; idle reaper is disabled"
        );
        return;
    }
    REAPER_STARTED.get_or_init(|| {
        info!(
            event = "whisper_residency_policy",
            effective_ttl_secs = policy.effective_ttl_secs,
            keep_warm = false,
            "Whisper residency idle reaper armed"
        );
        let spawned = std::thread::Builder::new()
            .name("whisper-idle-reaper".into())
            .spawn(reaper_loop);
        if let Err(e) = spawned {
            warn!(
                event = "whisper_residency_policy",
                effective_ttl_secs = policy.effective_ttl_secs,
                keep_warm = false,
                "Failed to spawn Whisper idle reaper: {e}"
            );
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
        let idle_for = guard.last_used.elapsed();
        let idle_ms = idle_for.as_millis().min(u128::from(u64::MAX)) as u64;
        if guard.engine.is_some() && idle_for >= threshold {
            // Drop weights only (LocalWhisperEngine). The process-cached Metal
            // Device in engine::process_device stays alive so the next cold load
            // reuses it — no Device::new_metal churn / port leak.
            let unload_started = Instant::now();
            guard.engine = None;
            let unload_drop_ms = unload_started.elapsed().as_millis() as u64;
            // Dropped weight buffers only return to the MetalDevice free-buffer
            // pool; force candle's prune or the multi-GB stays resident until
            // the NEXT inference. Done under the slot lock so a concurrent
            // reload cannot interleave with the pool sweep.
            let metal_reclaim_started = Instant::now();
            let metal_reclaim_attempted =
                if let Some(device) = super::engine::cached_process_device() {
                    crate::memory::reclaim_metal_buffer_pool(&device);
                    true
                } else {
                    false
                };
            let metal_reclaim_ms = metal_reclaim_started.elapsed().as_millis() as u64;
            drop(guard);
            let heap_release_started = Instant::now();
            crate::memory::release_freed_heap();
            let heap_release_ms = heap_release_started.elapsed().as_millis() as u64;
            let unload_count = RESIDENCY_UNLOAD_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            let reclaim_count = RESIDENCY_RECLAIM_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            info!(
                event = "whisper_residency_reclaim",
                unload_count,
                reclaim_count,
                effective_ttl_secs = threshold.as_secs(),
                keep_warm = false,
                idle_ms,
                unload_drop_ms,
                metal_reclaim_attempted,
                metal_reclaim_ms,
                heap_release_ms,
                "Whisper residency unload and reclaim complete"
            );
        }
    }
}

/// Run `f` with the engine, loading it on demand and refreshing the idle clock.
fn with_engine<R>(f: impl FnOnce(&mut LocalWhisperEngine) -> Result<R>) -> Result<R> {
    let lock_started = Instant::now();
    let mut guard = slot()
        .lock()
        .map_err(|e| anyhow!("Failed to lock engine: {}", e))?;
    let lock_wait_ms = lock_started.elapsed().as_millis() as u64;
    let mut model_load_ms = 0u64;
    let cold_load = guard.engine.is_none();
    if cold_load {
        let load_started = Instant::now();
        guard.engine = Some(load_engine()?);
        model_load_ms = load_started.elapsed().as_millis() as u64;
        ensure_reaper();
        record_residency_load(model_load_ms);
    }
    super::timing::record_engine_acquire(lock_wait_ms, model_load_ms, cold_load);
    guard.last_used = Instant::now();
    let engine = guard
        .engine
        .as_mut()
        .ok_or_else(|| anyhow!("Engine not initialized"))?;
    f(engine)
}

/// Run `f` with a per-call Whisper initial prompt installed on the engine.
///
/// The engine is shared, so the previous `initial_prompt` is captured and
/// restored afterwards — including when `f` fails — leaving no prompt bleed into
/// the next caller.
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

/// Full-file decoding is deliberately prompt-free. The live A/B measured a
/// vocabulary prompt deleting roughly half the file; lexicon voice belongs to
/// bounded tail/utterance windows only.
fn file_transcription_initial_prompt() -> Option<String> {
    None
}

/// Like [`with_engine`] but never blocks: if the engine is busy, return an error
/// instead of waiting. Used by best-effort correction passes.
fn try_with_engine<R>(f: impl FnOnce(&mut LocalWhisperEngine) -> Result<R>) -> Result<R> {
    let mut guard = slot()
        .try_lock()
        .map_err(|_| anyhow!("Whisper engine busy, skipping correction"))?;
    let mut model_load_ms = 0u64;
    let cold_load = guard.engine.is_none();
    if cold_load {
        let load_started = Instant::now();
        guard.engine = Some(load_engine()?);
        model_load_ms = load_started.elapsed().as_millis() as u64;
        ensure_reaper();
        record_residency_load(model_load_ms);
    }
    // try_lock never waits, so lock_wait is 0 by construction.
    super::timing::record_engine_acquire(0, model_load_ms, cold_load);
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
    #[cfg(test)]
    TEST_INIT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    with_engine(|_| Ok(()))
}

#[cfg(test)]
pub(crate) fn reset_test_init_calls() {
    TEST_INIT_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    TEST_LOAD_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn test_init_calls() -> usize {
    TEST_INIT_CALLS.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
pub(crate) fn test_load_calls() -> usize {
    TEST_LOAD_CALLS.load(std::sync::atomic::Ordering::SeqCst)
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

/// Whisper singleton: prompt opt-in, idle unload, model path precedence, load.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::ffi::OsString;

    /// RAII capture of one process env key for serial restoration on drop.
    struct EnvRestore {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvRestore {
        /// Snapshot `key`'s current value (or absence) before a test mutates it.
        fn capture(key: &'static str) -> Self {
            Self {
                key,
                previous: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        /// Restore the exact process env captured at `capture`.
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    /// File transcription stays prompt-free by contract.
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

    /// Window opt-in must never leak into full-file transcription.
    #[test]
    #[serial]
    fn file_transcription_initial_prompt_stays_off_when_window_prompt_is_opted_in() {
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

        assert_eq!(
            file_transcription_initial_prompt(),
            None,
            "full-file prompting is forbidden even when window prompting is enabled"
        );
    }

    /// Normal default is one minute: fp16 makes reload cheap enough that a
    /// multi-GB five-minute idle floor is no longer a good product tradeoff.
    #[test]
    #[serial]
    fn whisper_default_ttl_is_60() {
        let _ttl = EnvRestore::capture("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS");

        unsafe { std::env::remove_var("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS") };
        assert_eq!(
            idle_unload_after(),
            Some(Duration::from_secs(60)),
            "normal Whisper residency must default to 60 seconds"
        );
    }

    /// Supporting GREEN guard: runtime overrides remain effective, including
    /// explicit zero as the power-user keep-warm setting.
    #[test]
    #[serial]
    fn fleet_red_whisper_effective_ttl_overrides_include_zero_keep_warm() {
        let _ttl = EnvRestore::capture("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS");

        unsafe { std::env::set_var("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS", "17") };
        assert_eq!(idle_unload_after(), Some(Duration::from_secs(17)));

        unsafe { std::env::set_var("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS", "0") };
        assert_eq!(
            idle_unload_after(),
            None,
            "explicit zero is the power-user keep-warm override"
        );
    }

    #[test]
    #[serial]
    fn whisper_residency_policy_exposes_effective_ttl_and_keep_warm() {
        let _ttl = EnvRestore::capture("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS");

        unsafe { std::env::remove_var("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS") };
        assert_eq!(
            whisper_residency_policy(),
            WhisperResidencyPolicy {
                effective_ttl_secs: 60,
                keep_warm: false,
            }
        );

        unsafe { std::env::set_var("CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS", "0") };
        assert_eq!(
            whisper_residency_policy(),
            WhisperResidencyPolicy {
                effective_ttl_secs: 0,
                keep_warm: true,
            }
        );
    }

    /// LOCAL_MODEL precedence: process env > UserSettings > env file on disk.
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

    /// Resolved model path must load; empty PCM no-op stays empty (soft-skip if no model).
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
