//! Singleton pattern for embedder - easy global access.
//!
//! Provides a global embedder instance that is loaded on demand and reused.
//! Thread-safe via a single `Mutex` guarding a resettable slot.
//!
//! ## Idle unload
//!
//! Like Whisper, the MiniLM embedder lives on the GPU (Metal, candle BertModel)
//! and its multilingual tokenizer is a large host-side structure — together a
//! few hundred MB held for the whole process. The engine is held in a
//! *resettable* slot: after a configurable idle period a background reaper drops
//! **weights** only. The Candle Metal `Device` is process-cached so reload does
//! not recreate `Device::new_metal` (IOAccelerator port leak). Default TTL is
//! 45 minutes. Set `CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS=0` to disable.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{info, warn};

use super::engine::{EmbedderConfig, EmbedderEngine};

/// Default idle period after which embedder **weights** are unloaded (45 min).
/// Metal `Device` stays process-cached. Override with
/// `CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS` (`0` disables).
const DEFAULT_IDLE_UNLOAD_SECS: u64 = 2700;

/// How often the reaper wakes to check for idleness.
const REAPER_TICK: Duration = Duration::from_secs(30);

/// Resettable engine slot: `None` when unloaded, plus the last-use timestamp.
struct EmbedderSlot {
    /// Loaded engine, or `None` once the reaper has dropped its weights.
    engine: Option<EmbedderEngine>,
    /// Refreshed on every `with_embedder` call; the reaper's idle clock.
    last_used: Instant,
}

/// The one process-wide engine slot.
static SLOT: OnceLock<Mutex<EmbedderSlot>> = OnceLock::new();

/// Config used to (re)load the engine. First value wins (default unless
/// `init_with_config` set one before the first load).
static CONFIG: OnceLock<EmbedderConfig> = OnceLock::new();

/// Guard so the idle reaper thread is spawned at most once.
static REAPER_STARTED: OnceLock<()> = OnceLock::new();

/// Accessor for the engine slot, initialized empty on first use.
fn slot() -> &'static Mutex<EmbedderSlot> {
    SLOT.get_or_init(|| {
        Mutex::new(EmbedderSlot {
            engine: None,
            last_used: Instant::now(),
        })
    })
}

/// The config every (re)load uses. First value set wins for the process.
fn config() -> EmbedderConfig {
    CONFIG.get_or_init(EmbedderConfig::default).clone()
}

/// Resolve the configured idle-unload period, or `None` when disabled (0).
fn idle_unload_after() -> Option<Duration> {
    let secs = std::env::var("CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_UNLOAD_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Probe text for the backend self-test. Short, ASCII, and meaningless — it only
/// has to produce numbers.
const BACKEND_PROBE_TEXT: &str = "probe";

/// Build the engine and refuse to hand back one that cannot produce finite
/// vectors. On a non-finite accelerated backend the process is demoted to CPU
/// and the load retried once; a non-finite CPU result is a hard error.
fn load_engine() -> Result<EmbedderEngine> {
    let mut engine = EmbedderEngine::with_config(config())?;

    // Verify the backend produces finite vectors before anyone trusts it.
    //
    // Measured 2026-08-09 on macOS 27.0 / Metal: this exact model returned 384
    // dimensions of NaN for EVERY input, while the same weights on the CPU
    // returned unit-norm vectors with sensible similarities (cargo/kargo 0.836).
    // Nothing surfaced, because the sole consumer — the semantic dedup gate —
    // compares against a threshold, and every comparison with NaN is false: the
    // gate reported `gate_drops=0` across 378 real deliveries and read as
    // "nothing to drop" rather than "I am blind". A 471 MB model was loaded on
    // every delivery to compute nothing.
    //
    // The self-test costs one embedding at load time and converts a silent
    // wrong answer into a loud, self-healing one.
    let probe = engine.embed(BACKEND_PROBE_TEXT)?;
    let degenerate = probe.is_empty() || probe.iter().any(|value| !value.is_finite());
    if degenerate && !super::engine::is_demoted_to_cpu() {
        warn!(
            "Embedder backend returned non-finite output ({} dims); demoting this process to CPU and reloading",
            probe.len()
        );
        super::engine::demote_to_cpu();
        engine = EmbedderEngine::with_config(config())?;
        let retry = engine.embed(BACKEND_PROBE_TEXT)?;
        anyhow::ensure!(
            !retry.is_empty() && retry.iter().all(|value| value.is_finite()),
            "Embedder produced non-finite output on CPU as well; refusing to serve a blind semantic gate"
        );
        info!("Embedder recovered on CPU after a non-finite accelerated backend");
    } else if degenerate {
        anyhow::bail!(
            "Embedder produced non-finite output on CPU; refusing to serve a blind semantic gate"
        );
    }

    info!("Embedder engine loaded");
    Ok(engine)
}

/// Spawn the idle reaper once (only when idle-unload is enabled).
fn ensure_reaper() {
    if idle_unload_after().is_none() {
        return;
    }
    REAPER_STARTED.get_or_init(|| {
        let spawned = std::thread::Builder::new()
            .name("embedder-idle-reaper".into())
            .spawn(reaper_loop);
        if let Err(e) = spawned {
            warn!("Failed to spawn embedder idle reaper: {e}");
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
            // Weights only — process Metal device retained (see engine::process_device).
            guard.engine = None;
            // Force candle's free-pool prune so the dropped buffers leave RSS
            // now, not at the next inference (same mechanism as Whisper).
            if let Some(device) = super::engine::cached_process_device() {
                crate::memory::reclaim_metal_buffer_pool(&device);
            }
            drop(guard);
            info!(
                "Embedder weights unloaded after {}s idle (Metal device retained, buffer pool pruned); releasing host heap",
                threshold.as_secs()
            );
            crate::memory::release_freed_heap();
        }
    }
}

/// Run `f` with the engine, loading it on demand and refreshing the idle clock.
fn with_embedder<R>(f: impl FnOnce(&mut EmbedderEngine) -> Result<R>) -> Result<R> {
    let mut guard = slot()
        .lock()
        .map_err(|e| anyhow::anyhow!("Embedder lock poisoned: {}", e))?;
    if guard.engine.is_none() {
        guard.engine = Some(load_engine()?);
        ensure_reaper();
    }
    guard.last_used = Instant::now();
    let engine = guard
        .engine
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("Embedder not initialized"))?;
    f(engine)
}

/// Initialize the embedder with default config.
pub fn init() -> Result<()> {
    with_embedder(|_| Ok(()))
}

/// Initialize with custom configuration.
///
/// The config is captured for (re)loads; the first config wins. Idempotent.
pub fn init_with_config(config: EmbedderConfig) -> Result<()> {
    let _ = CONFIG.set(config);
    with_embedder(|_| Ok(()))
}

/// Check if the embedder is currently loaded.
///
/// Note: with idle-unload enabled this can become `false` again after a period
/// of inactivity; the next call reloads transparently.
pub fn is_initialized() -> bool {
    SLOT.get()
        .and_then(|m| m.lock().ok().map(|g| g.engine.is_some()))
        .unwrap_or(false)
}

/// Embed a single text (query)
///
/// Auto-initializes with default config if not already done.
pub fn embed(text: &str) -> Result<Vec<f32>> {
    with_embedder(|engine| engine.embed(text))
}

/// Embed a passage (document) for indexing
pub fn embed_passage(text: &str) -> Result<Vec<f32>> {
    with_embedder(|engine| engine.embed_passage(text))
}

/// Embed multiple texts at once
pub fn embed_batch(texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    with_embedder(|engine| engine.embed_batch(texts))
}

/// Embed multiple passages at once
pub fn embed_passages(texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    with_embedder(|engine| engine.embed_passages(texts))
}

/// Calculate cosine similarity between two embeddings
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    EmbedderEngine::similarity(a, b)
}

/// Get embedding dimension for the current model
pub fn dimension() -> Result<usize> {
    with_embedder(|engine| Ok(engine.dimension()))
}

/// Similarity math and idle-unload env parsing (no model download required).
#[cfg(test)]
mod tests {
    use super::*;

    /// Identical vectors must score cosine similarity ≈ 1.0.
    #[test]
    fn test_similarity_function() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    /// `CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS=0` disables unload; unset → 45 min.
    #[test]
    fn idle_unload_disabled_when_zero() {
        // SAFETY: single-threaded test mutating a process env var it owns.
        unsafe { std::env::set_var("CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS", "0") };
        assert!(idle_unload_after().is_none());
        unsafe { std::env::set_var("CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS", "90") };
        assert_eq!(idle_unload_after(), Some(Duration::from_secs(90)));
        unsafe { std::env::remove_var("CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS") };
        // Default is 45 min weight-only unload (Metal device stays process-cached).
        assert_eq!(idle_unload_after(), Some(Duration::from_secs(2700)));
    }

    // Note: Full embedding tests require model download and are in integration tests
}
