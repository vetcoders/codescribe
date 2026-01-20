//! Global Whisper engine singleton - model welded to the process.
//!
//! Release builds: Model bytes are embedded in binary, loaded directly to GPU.
//! Zero disk I/O, zero temp files, zero extraction.
//!
//! Debug builds: Uses CODESCRIBE_MODEL_PATH or bundled .app model.
//!
//! Created by M&K (c)2026 VetCoders

// This entire module is a public API for library consumers
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow};
use tracing::{info, warn};

use crate::config::Config;
use crate::config::models::ModelManager;

use super::engine::LocalWhisperEngine;
use super::params::DecodingParams;

/// Default model name (for dev/fallback mode)
pub const DEFAULT_MODEL: &str = "whisper-large-v3-turbo-mlx-q8";

/// Global singleton engine
static ENGINE: OnceLock<Mutex<LocalWhisperEngine>> = OnceLock::new();

/// Model path - only used for non-embedded (dev) mode
static MODEL_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Resolve the model path for dev/fallback mode
///
/// Only called when embedded model is NOT available.
fn resolve_model_path_fallback() -> Result<PathBuf> {
    // 1. Dev override
    if let Ok(path) = std::env::var("CODESCRIBE_MODEL_PATH") {
        let p = PathBuf::from(&path);
        if p.join("tokenizer.json").exists() {
            info!(
                "DEV: Using model from CODESCRIBE_MODEL_PATH: {}",
                p.display()
            );
            return Ok(p);
        }
        warn!("CODESCRIBE_MODEL_PATH set but model incomplete: {}", path);
    }

    // 2. Configured model (LOCAL_MODEL)
    let config = Config::load();
    let configured_model = config.local_model;
    if !configured_model.trim().is_empty()
        && let Ok(manager) = ModelManager::new()
    {
        let candidate = manager.get_model_path(&configured_model);
        if candidate.join("tokenizer.json").exists() {
            info!("Using configured model: {}", candidate.display());
            return Ok(candidate);
        }
    }

    // 3. Bundled .app fallback (Tauri builds without embedding)
    let exe = std::env::current_exe().context("Failed to get executable path")?;
    let exe_dir = exe.parent().context("Failed to get executable directory")?;

    let bundled_path = exe_dir.join("../Resources/models").join(DEFAULT_MODEL);

    if bundled_path.join("tokenizer.json").exists() {
        let canonical = bundled_path.canonicalize().unwrap_or(bundled_path);
        info!("Using bundled model: {}", canonical.display());
        return Ok(canonical);
    }

    Err(anyhow!(
        "Whisper model not available.\n\
         Debug builds: Set CODESCRIBE_MODEL_PATH\n\
         Release builds: Model should be embedded\n\n\
         Download with: ./scripts/download-model.sh"
    ))
}

/// Get the resolved model path (only for non-embedded mode)
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

/// Initialize the global engine (call once at startup)
///
/// Uses embedded model if available (zero I/O), otherwise falls back to path-based loading.
pub fn init() -> Result<()> {
    // 1. Embedded model (release builds) - ZERO DISK I/O
    //    Model bytes → GPU tensors, no temp files
    if let Some(embedded) = super::embedded::get_embedded_data() {
        let engine = LocalWhisperEngine::from_embedded(&embedded)
            .context("Failed to initialize from embedded model")?;

        ENGINE
            .set(Mutex::new(engine))
            .map_err(|_| anyhow!("Engine already initialized"))?;

        info!("Whisper engine initialized from embedded model (zero I/O)");
        return Ok(());
    }

    // 2. Fallback to path-based loading (dev mode, bundled .app)
    let path = get_model_path()?;
    let engine = LocalWhisperEngine::new_with_params(path, DecodingParams::default())
        .context("Failed to initialize Whisper engine from path")?;

    ENGINE
        .set(Mutex::new(engine))
        .map_err(|_| anyhow!("Engine already initialized"))?;

    info!("Whisper engine initialized from path: {}", path.display());
    Ok(())
}

/// Check if engine is initialized
pub fn is_initialized() -> bool {
    ENGINE.get().is_some()
}

/// Get the global engine (initializes on first call if needed)
pub fn engine() -> Result<&'static Mutex<LocalWhisperEngine>> {
    if !is_initialized() {
        init()?;
    }
    ENGINE
        .get()
        .ok_or_else(|| anyhow!("Engine not initialized"))
}

/// Transcribe audio samples using the global engine
pub fn transcribe(samples: &[f32], sample_rate: u32, language: Option<&str>) -> Result<String> {
    let engine_mutex = engine()?;
    let mut engine = engine_mutex
        .lock()
        .map_err(|e| anyhow!("Failed to lock engine: {}", e))?;

    engine.transcribe_long_with_language(samples, sample_rate, language)
}

/// Transcribe with streaming callback
pub fn transcribe_streaming<'a>(
    samples: &[f32],
    sample_rate: u32,
    language: Option<&str>,
    callback: Option<super::engine::ChunkCallback<'a>>,
) -> Result<String> {
    let engine_mutex = engine()?;
    let mut engine = engine_mutex
        .lock()
        .map_err(|e| anyhow!("Failed to lock engine: {}", e))?;

    engine.transcribe_long_streaming(samples, sample_rate, language, callback)
}

/// Transcribe a file
pub fn transcribe_file(path: &std::path::Path, language: Option<&str>) -> Result<String> {
    let (samples, sample_rate) =
        crate::audio::load_audio_file(path).context("Failed to load audio file")?;

    transcribe(&samples, sample_rate, language)
}

/// Detect language from audio samples
pub fn detect_language(samples: &[f32], sample_rate: u32) -> Result<String> {
    let engine_mutex = engine()?;
    let mut engine = engine_mutex
        .lock()
        .map_err(|e| anyhow!("Failed to lock engine: {}", e))?;

    engine.detect_language(samples, sample_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_path_resolution() {
        // This test verifies the path resolution logic works
        // It may or may not find a model depending on environment
        let result = resolve_model_path_fallback();

        // In CI without model, this will fail - that's expected
        if let Ok(path) = result {
            assert!(path.join("tokenizer.json").exists());
            println!("Found model at: {}", path.display());
        } else {
            println!("No model found (expected in CI): {:?}", result.err());
        }
    }
}
