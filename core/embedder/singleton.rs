//! Singleton pattern for embedder - easy global access.
//!
//! Provides a global embedder instance that is loaded on demand and reused.
//! Thread-safe via a single `Mutex` guarding a resettable slot.
//!
//! ## Idle unload
//!
//! Like Whisper, the MiniLM embedder lives on the GPU (Metal, candle BertModel)
//! and its multilingual tokenizer is a large host-side structure — together a
//! few hundred MB held for the whole process. The engine is therefore held in a
//! *resettable* slot: after a configurable idle period with no embedding a
//! background reaper drops it, returning GPU/host memory to the system, and the
//! next call transparently reloads it. Set
//! `CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS=0` to disable.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{info, warn};

use super::engine::{EmbedderConfig, EmbedderEngine};

/// Default idle period after which the embedder is unloaded to free GPU memory.
/// Disabled by default (0): like the Whisper engine, idle-unload recreates the
/// Metal device on reload and leaks IOAccelerator ports/threads per cycle.
/// Re-enable per machine via `CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS=<secs>`.
const DEFAULT_IDLE_UNLOAD_SECS: u64 = 0;

/// How often the reaper wakes to check for idleness.
const REAPER_TICK: Duration = Duration::from_secs(30);

/// Resettable engine slot: `None` when unloaded, plus the last-use timestamp.
struct EmbedderSlot {
    engine: Option<EmbedderEngine>,
    last_used: Instant,
}

static SLOT: OnceLock<Mutex<EmbedderSlot>> = OnceLock::new();

/// Config used to (re)load the engine. First value wins (default unless
/// `init_with_config` set one before the first load).
static CONFIG: OnceLock<EmbedderConfig> = OnceLock::new();

/// Guard so the idle reaper thread is spawned at most once.
static REAPER_STARTED: OnceLock<()> = OnceLock::new();

fn slot() -> &'static Mutex<EmbedderSlot> {
    SLOT.get_or_init(|| {
        Mutex::new(EmbedderSlot {
            engine: None,
            last_used: Instant::now(),
        })
    })
}

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

fn load_engine() -> Result<EmbedderEngine> {
    let engine = EmbedderEngine::with_config(config())?;
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
            guard.engine = None;
            drop(guard);
            info!(
                "Embedder engine unloaded after {}s idle; releasing GPU/host memory",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_similarity_function() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn idle_unload_disabled_when_zero() {
        // SAFETY: single-threaded test mutating a process env var it owns.
        unsafe { std::env::set_var("CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS", "0") };
        assert!(idle_unload_after().is_none());
        unsafe { std::env::set_var("CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS", "90") };
        assert_eq!(idle_unload_after(), Some(Duration::from_secs(90)));
        unsafe { std::env::remove_var("CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS") };
        // DEFAULT_IDLE_UNLOAD_SECS is now 0 (idle-unload disabled by default),
        // so with no override the reaper is off.
        assert!(idle_unload_after().is_none());
    }

    // Note: Full embedding tests require model download and are in integration tests
}
