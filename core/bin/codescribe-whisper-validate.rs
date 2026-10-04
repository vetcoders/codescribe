//! Validate a composed Whisper model with the runtime's canonical contract.

use anyhow::{Context, Result, anyhow};
use codescribe_core::config::models::{
    resolve_runtime_whisper_model_path, validate_whisper_model_bundle,
};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let command = args
        .next()
        .ok_or_else(|| anyhow!("usage: codescribe-whisper-validate <model-directory>|--resolve"))?;
    if args.next().is_some() {
        return Err(anyhow!(
            "usage: codescribe-whisper-validate <model-directory>|--resolve"
        ));
    }

    if command == "--resolve" {
        println!("{}", resolve_runtime_whisper_model_path(None)?.display());
        return Ok(());
    }

    let path = PathBuf::from(command);
    validate_whisper_model_bundle(&path)
        .with_context(|| format!("invalid Whisper model bundle: {}", path.display()))
}
