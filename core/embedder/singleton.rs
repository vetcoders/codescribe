//! Singleton pattern for embedder - easy global access.
//!
//! Provides a global embedder instance that's initialized once and reused.
//! Thread-safe via OnceLock + Mutex pattern.

use std::sync::{Mutex, OnceLock};

use anyhow::Result;
use tracing::info;

use super::engine::{EmbedderConfig, EmbedderEngine};

/// Global embedder instance
static EMBEDDER_INSTANCE: OnceLock<Mutex<EmbedderEngine>> = OnceLock::new();

/// Initialize the embedder with default config
pub fn init() -> Result<()> {
    init_with_config(EmbedderConfig::default())
}

/// Initialize with custom configuration
pub fn init_with_config(config: EmbedderConfig) -> Result<()> {
    if EMBEDDER_INSTANCE.get().is_some() {
        info!("Embedder already initialized");
        return Ok(());
    }

    let engine = EmbedderEngine::with_config(config)?;

    EMBEDDER_INSTANCE
        .set(Mutex::new(engine))
        .map_err(|_| anyhow::anyhow!("Embedder already initialized"))?;

    info!("Embedder singleton initialized");
    Ok(())
}

/// Check if embedder is initialized
pub fn is_initialized() -> bool {
    EMBEDDER_INSTANCE.get().is_some()
}

/// Embed a single text (query)
///
/// Auto-initializes with default config if not already done.
pub fn embed(text: &str) -> Result<Vec<f32>> {
    ensure_initialized()?;

    let embedder = EMBEDDER_INSTANCE
        .get()
        .ok_or_else(|| anyhow::anyhow!("Embedder not initialized"))?;

    let mut guard = embedder
        .lock()
        .map_err(|e| anyhow::anyhow!("Embedder lock poisoned: {}", e))?;

    guard.embed(text)
}

/// Embed a passage (document) for indexing
pub fn embed_passage(text: &str) -> Result<Vec<f32>> {
    ensure_initialized()?;

    let embedder = EMBEDDER_INSTANCE
        .get()
        .ok_or_else(|| anyhow::anyhow!("Embedder not initialized"))?;

    let mut guard = embedder
        .lock()
        .map_err(|e| anyhow::anyhow!("Embedder lock poisoned: {}", e))?;

    guard.embed_passage(text)
}

/// Embed multiple texts at once
pub fn embed_batch(texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    ensure_initialized()?;

    let embedder = EMBEDDER_INSTANCE
        .get()
        .ok_or_else(|| anyhow::anyhow!("Embedder not initialized"))?;

    let mut guard = embedder
        .lock()
        .map_err(|e| anyhow::anyhow!("Embedder lock poisoned: {}", e))?;

    guard.embed_batch(texts)
}

/// Embed multiple passages at once
pub fn embed_passages(texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    ensure_initialized()?;

    let embedder = EMBEDDER_INSTANCE
        .get()
        .ok_or_else(|| anyhow::anyhow!("Embedder not initialized"))?;

    let mut guard = embedder
        .lock()
        .map_err(|e| anyhow::anyhow!("Embedder lock poisoned: {}", e))?;

    guard.embed_passages(texts)
}

/// Calculate cosine similarity between two embeddings
pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
    EmbedderEngine::similarity(a, b)
}

/// Get embedding dimension for the current model
pub fn dimension() -> Result<usize> {
    ensure_initialized()?;

    let embedder = EMBEDDER_INSTANCE
        .get()
        .ok_or_else(|| anyhow::anyhow!("Embedder not initialized"))?;

    let guard = embedder
        .lock()
        .map_err(|e| anyhow::anyhow!("Embedder lock poisoned: {}", e))?;

    Ok(guard.dimension())
}

/// Ensure embedder is initialized (auto-init with defaults if not)
fn ensure_initialized() -> Result<()> {
    if !is_initialized() {
        init()?;
    }
    Ok(())
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

    // Note: Full embedding tests require model download and are in integration tests
}
