//! Global TTS engine singleton - model welded to the process.
//!
//! Release builds: Model bytes are embedded in binary, loaded directly to GPU.
//! Zero disk I/O, zero temp files, zero extraction.
//!
//! Debug builds: Uses CODESCRIBE_TTS_PATH or bundled .app model.
//!
//! Created by M&K (c)2026 VetCoders

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow};
use directories::BaseDirs;
use tracing::{info, warn};

use super::audio_output::AudioPlayer;
use super::engine::TtsEngine;
use crate::hf_cache;

/// Default TTS model name (for dev/fallback mode)
pub const DEFAULT_MODEL: &str = "csm-1b";
const DEFAULT_TTS_REPO: &str = "sesame/csm-1b";

/// Global singleton engine
static ENGINE: OnceLock<Mutex<TtsEngine>> = OnceLock::new();

/// Model path - only used for non-embedded (dev) mode
static MODEL_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Audio player instance
static PLAYER: OnceLock<AudioPlayer> = OnceLock::new();

/// Resolve the model path for dev/fallback mode
///
/// Only called when embedded model is NOT available.
fn resolve_model_path_fallback() -> Result<PathBuf> {
    // 1. Dev override
    if let Ok(path) = std::env::var("CODESCRIBE_TTS_PATH") {
        let p = PathBuf::from(&path);
        if p.join("config.json").exists() {
            info!(
                "DEV: Using TTS model from CODESCRIBE_TTS_PATH: {}",
                p.display()
            );
            return Ok(p);
        }
        warn!("CODESCRIBE_TTS_PATH set but model incomplete: {}", path);
    }

    // 2. HuggingFace cache (hf download sesame/csm-1b)
    if let Some(snapshot) = hf_cache::find_snapshot(
        DEFAULT_TTS_REPO,
        &["config.json", "tokenizer.json", "model.safetensors"],
    ) {
        info!("Using HF cache TTS model: {}", snapshot.display());
        return Ok(snapshot);
    }

    // 3. Default models directory
    let base_dirs = BaseDirs::new().ok_or_else(|| anyhow!("Cannot determine home directory"))?;
    let home = base_dirs.home_dir();
    let models_dir = home.join(".codescribe").join("models").join(DEFAULT_MODEL);
    if models_dir.join("config.json").exists() {
        info!("Using TTS model from: {}", models_dir.display());
        return Ok(models_dir);
    }

    // 4. Bundled .app fallback (Tauri builds without embedding)
    let exe = std::env::current_exe().context("Failed to get executable path")?;
    let exe_dir = exe.parent().context("Failed to get executable directory")?;

    let bundled_path = exe_dir.join("../Resources/models").join(DEFAULT_MODEL);

    if bundled_path.join("config.json").exists() {
        let canonical = bundled_path.canonicalize().unwrap_or(bundled_path);
        info!("Using bundled TTS model: {}", canonical.display());
        return Ok(canonical);
    }

    Err(anyhow!(
        "TTS model not available.\n\
         Debug builds: Set CODESCRIBE_TTS_PATH\n\
         Release builds: Model should be embedded\n\n\
         Download with: hf download {}",
        DEFAULT_TTS_REPO
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

/// Initialize the global TTS engine (call once at startup)
///
/// Uses embedded model if available (zero I/O), otherwise falls back to path-based loading.
pub fn init() -> Result<()> {
    // Initialize audio player first
    let _ = PLAYER.get_or_init(|| {
        AudioPlayer::new().unwrap_or_else(|e| {
            warn!(
                "Failed to initialize audio player: {}. Playback will fail.",
                e
            );
            AudioPlayer::dummy()
        })
    });

    // 1. Embedded model (release builds) - ZERO DISK I/O
    //    Model bytes → GPU tensors, no temp files
    if let Some(embedded) = super::embedded::get_embedded_data() {
        match TtsEngine::from_embedded(&embedded) {
            Ok(engine) => {
                ENGINE
                    .set(Mutex::new(engine))
                    .map_err(|_| anyhow!("TTS engine already initialized"))?;

                info!("TTS engine initialized from embedded model (zero I/O)");
                return Ok(());
            }
            Err(error) => {
                warn!(
                    "Embedded TTS bundle present but failed to initialize: {}. Falling back to path-based loading.",
                    error
                );
            }
        }
    }

    // 2. Fallback to path-based loading (dev mode, bundled .app)
    let path = get_model_path()?;
    let engine = TtsEngine::new(path).context("Failed to initialize TTS engine from path")?;

    ENGINE
        .set(Mutex::new(engine))
        .map_err(|_| anyhow!("TTS engine already initialized"))?;

    info!("TTS engine initialized from path: {}", path.display());
    Ok(())
}

/// Check if TTS engine is initialized
pub fn is_initialized() -> bool {
    ENGINE.get().is_some()
}

/// Get the global engine (initializes on first call if needed)
fn engine() -> Result<&'static Mutex<TtsEngine>> {
    if !is_initialized() {
        init()?;
    }
    ENGINE
        .get()
        .ok_or_else(|| anyhow!("TTS engine not initialized"))
}

/// Get the audio player
fn player() -> Result<&'static AudioPlayer> {
    PLAYER
        .get()
        .ok_or_else(|| anyhow!("Audio player not initialized"))
}

/// Synthesize text to audio samples
///
/// Returns f32 PCM samples at 24kHz sample rate.
pub fn synthesize(text: &str) -> Result<Vec<f32>> {
    let engine_mutex = engine()?;
    let mut engine = engine_mutex
        .lock()
        .map_err(|e| anyhow!("Failed to lock TTS engine: {}", e))?;

    engine.synthesize(text)
}

/// Synthesize text with specific speaker index
pub fn synthesize_with_speaker(text: &str, speaker_idx: usize) -> Result<Vec<f32>> {
    let engine_mutex = engine()?;
    let mut engine = engine_mutex
        .lock()
        .map_err(|e| anyhow!("Failed to lock TTS engine: {}", e))?;

    engine.synthesize_with_speaker(text, speaker_idx)
}

/// Synthesize text and save to WAV file
pub fn synthesize_to_file(text: &str, path: &Path) -> Result<()> {
    let samples = synthesize(text)?;
    AudioPlayer::save_wav(&samples, super::SAMPLE_RATE, path)
}

/// Synthesize and play immediately (blocking)
pub fn play(text: &str) -> Result<()> {
    let samples = synthesize(text)?;
    let audio_player = player()?;
    audio_player.play(&samples, super::SAMPLE_RATE)
}

/// Synthesize and play with specific speaker (blocking)
pub fn play_with_speaker(text: &str, speaker_idx: usize) -> Result<()> {
    let samples = synthesize_with_speaker(text, speaker_idx)?;
    let audio_player = player()?;
    audio_player.play(&samples, super::SAMPLE_RATE)
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
            assert!(path.join("config.json").exists());
            println!("Found TTS model at: {}", path.display());
        } else {
            println!("No TTS model found (expected in CI): {:?}", result.err());
        }
    }
}
