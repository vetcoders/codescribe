//! Global Whisper engine singleton - model as part of the process.
//!
//! The model is loaded once and kept in memory. No dynamic path searching.
//! Works for both bundled .app and development mode.
//!
//! Created by M&K (c)2026 VetCoders

// This entire module is a public API for library consumers
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow};
use tracing::{info, warn};

use super::engine::LocalWhisperEngine;
use super::params::DecodingParams;

/// Default model name
pub const DEFAULT_MODEL: &str = "whisper-large-v3-turbo-mlx-q8";

/// Global singleton engine
static ENGINE: OnceLock<Mutex<LocalWhisperEngine>> = OnceLock::new();

/// Model path - resolved once at startup
static MODEL_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Resolve the model path (called once)
///
/// Priority:
/// 1. Bundled .app: Contents/Resources/models/{model}
/// 2. Development: ./models/{model}
/// 3. CODESCRIBE_MODEL_PATH env override
fn resolve_model_path() -> Result<PathBuf> {
    // Environment override (for testing/custom setups)
    if let Ok(path) = std::env::var("CODESCRIBE_MODEL_PATH") {
        let p = PathBuf::from(&path);
        if p.join("tokenizer.json").exists() {
            info!("Using model from CODESCRIBE_MODEL_PATH: {}", p.display());
            return Ok(p);
        }
        warn!("CODESCRIBE_MODEL_PATH set but model incomplete: {}", path);
    }

    let exe = std::env::current_exe().context("Failed to get executable path")?;
    let exe_dir = exe.parent().context("Failed to get executable directory")?;

    // 1. Bundled .app: Contents/MacOS/binary -> Contents/Resources/models/
    let bundled_path = exe_dir
        .join("../Resources/models")
        .join(DEFAULT_MODEL);

    if bundled_path.join("tokenizer.json").exists() {
        let canonical = bundled_path.canonicalize().unwrap_or(bundled_path);
        info!("Using bundled model: {}", canonical.display());
        return Ok(canonical);
    }

    // 2. Development: ./models/ relative to repo root
    //    exe is in target/debug/ or target/release/, models is at repo root
    let dev_candidates = [
        // From target/debug/codescribe -> ../../models/
        exe_dir.join("../../models").join(DEFAULT_MODEL),
        // Direct ./models/ (running from repo root)
        PathBuf::from("models").join(DEFAULT_MODEL),
    ];

    for candidate in &dev_candidates {
        if candidate.join("tokenizer.json").exists() {
            let canonical = candidate.canonicalize().unwrap_or_else(|_| candidate.clone());
            info!("Using development model: {}", canonical.display());
            return Ok(canonical);
        }
    }

    Err(anyhow!(
        "Whisper model '{}' not found.\n\
         Searched:\n\
         - Bundled: {}\n\
         - Dev: {:?}\n\n\
         Download with: ./scripts/download_models.sh",
        DEFAULT_MODEL,
        bundled_path.display(),
        dev_candidates
    ))
}

/// Get the resolved model path
pub fn get_model_path() -> Result<&'static PathBuf> {
    // Try to get existing path
    if let Some(path) = MODEL_PATH.get() {
        return Ok(path);
    }

    // Resolve and store
    let path = resolve_model_path()?;
    let _ = MODEL_PATH.set(path.clone());

    // Return the stored path (handles race condition)
    MODEL_PATH
        .get()
        .ok_or_else(|| anyhow!("Failed to store model path"))
}

/// Initialize the global engine (call once at startup)
pub fn init() -> Result<()> {
    let path = get_model_path()?;

    let engine = LocalWhisperEngine::new_with_params(path, DecodingParams::default())
        .context("Failed to initialize Whisper engine")?;

    ENGINE
        .set(Mutex::new(engine))
        .map_err(|_| anyhow!("Engine already initialized"))?;

    info!("Whisper engine initialized and ready");
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
    ENGINE.get().ok_or_else(|| anyhow!("Engine not initialized"))
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
    let (samples, sample_rate) = crate::audio_loader::load_audio_file(path)
        .context("Failed to load audio file")?;

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
        let result = resolve_model_path();

        // In CI without model, this will fail - that's expected
        if let Ok(path) = result {
            assert!(path.join("tokenizer.json").exists());
            println!("Found model at: {}", path.display());
        } else {
            println!("No model found (expected in CI): {:?}", result.err());
        }
    }
}
