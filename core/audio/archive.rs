//! Compressed archive copies of recordings.
//!
//! Kept apart from the live path on purpose: the realtime pipeline and the
//! final pass both read raw WAV, and only the copy kept for later listening is
//! worth the encode. Encoding shells out to the system `afconvert` rather than
//! linking a codec, so the binary carries no encoder weight.

use anyhow::{Context, Result, anyhow};
use std::path::Path;
use std::process::Command;

const AFCONVERT: &str = "/usr/bin/afconvert";
const AAC_BITRATE_BPS: &str = "64000";

/// Encode a recording archive copy as AAC inside an m4a container.
///
/// This is intentionally archive-only: live recording still writes raw WAV
/// files for the realtime pipeline and final-pass transcription.
pub(crate) fn encode_wav_to_m4a(src_path: &Path, dest_path: &Path) -> Result<()> {
    if !src_path.exists() {
        return Err(anyhow!("source WAV does not exist: {}", src_path.display()));
    }
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create archive directory {}", parent.display()))?;
    }

    encode_wav_to_m4a_platform(src_path, dest_path)
}

/// macOS encode via `afconvert`. A non-zero exit is surfaced with the tool's
/// own stderr attached, so a codec refusal reads as itself rather than as a
/// generic failure.
#[cfg(target_os = "macos")]
fn encode_wav_to_m4a_platform(src_path: &Path, dest_path: &Path) -> Result<()> {
    let output = Command::new(AFCONVERT)
        .args(["-f", "m4af", "-d", "aac@44100", "-b", AAC_BITRATE_BPS])
        .arg(src_path)
        .arg(dest_path)
        .output()
        .context("spawn afconvert for m4a archive encoding")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "afconvert failed with status {}: {}",
        output.status,
        stderr.trim()
    ))
}

/// Non-macOS stub: fails loudly instead of silently skipping the archive, so
/// a missing archive copy can never be mistaken for a successful one.
#[cfg(not(target_os = "macos"))]
fn encode_wav_to_m4a_platform(_src_path: &Path, _dest_path: &Path) -> Result<()> {
    Err(anyhow!("m4a archive encoding requires macOS afconvert"))
}
