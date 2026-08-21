//! Validate a composed Whisper model with the runtime's canonical contract.

use anyhow::{Context, Result, anyhow};
use codescribe_core::config::models::validate_whisper_model_bundle;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("usage: codescribe-whisper-validate <model-directory>"))?;
    if args.next().is_some() {
        return Err(anyhow!(
            "usage: codescribe-whisper-validate <model-directory>"
        ));
    }

    validate_whisper_model_bundle(&path)
        .with_context(|| format!("invalid Whisper model bundle: {}", path.display()))
}
