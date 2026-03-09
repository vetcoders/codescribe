//! Silero VAD model installation helpers.
//!
//! Goal: make it easy to get `silero_vad.onnx` onto the machine without
//! embedding it into the binary.
//!
//! Created by M&K (c)2026 VetCoders

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Silero VAD v6 ONNX model URL.
pub const SILERO_VAD_URL: &str =
    "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx";

/// Model filename (as expected by the loader).
pub const SILERO_VAD_FILE: &str = "silero_vad.onnx";

/// Legacy/user models dir: `~/.codescribe/models/`.
pub fn user_models_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codescribe")
        .join("models")
}

/// Legacy/user model path: `~/.codescribe/models/silero_vad.onnx`.
pub fn user_model_path() -> PathBuf {
    user_models_dir().join(SILERO_VAD_FILE)
}

/// Download Silero VAD model to `~/.codescribe/models/silero_vad.onnx` if missing.
///
/// Returns the destination path (even if it already existed).
pub async fn ensure_downloaded_to_user_dir() -> Result<PathBuf> {
    let dest = user_model_path();
    if dest.exists() {
        return Ok(dest);
    }

    let parent = dest
        .parent()
        .context("Silero VAD destination has no parent dir")?;
    tokio::fs::create_dir_all(parent)
        .await
        .context("Failed to create ~/.codescribe/models directory")?;

    download_silero_vad(&dest).await?;
    Ok(dest)
}

/// Download the Silero VAD model from the hardcoded upstream URL.
async fn download_silero_vad(dest: &Path) -> Result<()> {
    let tmp = dest.with_extension("onnx.part");

    let resp = reqwest::get(SILERO_VAD_URL)
        .await
        .with_context(|| format!("Failed to GET {}", SILERO_VAD_URL))?
        .error_for_status()
        .with_context(|| format!("HTTP error downloading {}", SILERO_VAD_URL))?;

    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("Failed to read body from {}", SILERO_VAD_URL))?;

    if bytes.is_empty() {
        anyhow::bail!("Downloaded file is empty: {}", SILERO_VAD_URL);
    }

    tokio::fs::write(&tmp, &bytes)
        .await
        .with_context(|| format!("Failed to write {}", tmp.display()))?;

    // Atomic-ish replace on macOS when staying on same filesystem.
    tokio::fs::rename(&tmp, dest)
        .await
        .with_context(|| format!("Failed to move {} -> {}", tmp.display(), dest.display()))?;

    Ok(())
}
