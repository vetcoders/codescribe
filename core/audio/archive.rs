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

#[cfg(not(target_os = "macos"))]
fn encode_wav_to_m4a_platform(_src_path: &Path, _dest_path: &Path) -> Result<()> {
    Err(anyhow!("m4a archive encoding requires macOS afconvert"))
}
