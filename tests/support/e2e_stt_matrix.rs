//! Shared helpers for STT E2E matrix tests.
//!
//! Goals:
//! - Keep heavy tests explicitly opt-in (`CODESCRIBE_E2E_*`).
//! - Keep deterministic checks always-on.
//! - Reuse one model discovery strategy across E2E suites.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub const STT_OPT_IN_ENV: &str = "CODESCRIBE_E2E_STT";
pub const ROUNDTRIP_OPT_IN_ENV: &str = "CODESCRIBE_E2E_ROUNDTRIP";

/// Default composed fp16 alias.
pub const WHISPER_FP16_MODEL: &str = "whisper-large-v3-turbo";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    EnvOverride,
    ModelsDir,
    RuntimeResolver,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDiscovery {
    pub source: ModelSource,
    pub path: PathBuf,
}

pub fn parse_opt_in(value: Option<&str>) -> bool {
    value
        .map(str::trim)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn env_opt_in(key: &str) -> bool {
    parse_opt_in(std::env::var(key).ok().as_deref())
}

pub fn skip_unless_opt_in(key: &str, suite: &str, why: &str) -> bool {
    if env_opt_in(key) {
        return false;
    }

    eprintln!(
        "Skipping {} (opt-in heavy test: set {}=1). {}",
        suite, key, why
    );
    true
}

pub fn test_audio_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/assets/1.fretka-Ziggy.mp3")
}

pub fn whisper_model_is_complete(path: &Path) -> bool {
    codescribe_core::config::models::validate_whisper_model_bundle(path).is_ok()
}

pub fn whisper_model_missing_parts(path: &Path) -> Vec<&'static str> {
    let mut missing = Vec::new();

    if !path.join("config.json").exists() {
        missing.push("config.json");
    }
    if !path.join("tokenizer.json").exists() {
        missing.push("tokenizer.json");
    }
    if !path.join("mel_filters.npz").exists() {
        missing.push("mel_filters.npz");
    }
    let has_weights =
        path.join("weights.safetensors").exists() || path.join("model.safetensors").exists();
    if !has_weights {
        missing.push("weights.safetensors|model.safetensors");
    }

    missing
}

pub fn discover_local_whisper_model() -> Option<ModelDiscovery> {
    codescribe_core::config::models::resolve_runtime_whisper_model_path(None)
        .ok()
        .map(|path| ModelDiscovery {
            source: ModelSource::RuntimeResolver,
            path,
        })
}

pub fn expand_models_root(home_dir: &Path, value: &str) -> PathBuf {
    value
        .strip_prefix("~/")
        .map_or_else(|| PathBuf::from(value), |relative| home_dir.join(relative))
}

pub fn discover_local_whisper_model_for(
    home_dir: &Path,
    env_override: Option<&Path>,
) -> Option<ModelDiscovery> {
    discover_local_whisper_model_for_with_root(home_dir, env_override, None)
}

pub fn discover_local_whisper_model_for_with_root(
    home_dir: &Path,
    env_override: Option<&Path>,
    models_root: Option<&Path>,
) -> Option<ModelDiscovery> {
    if let Some(path) = env_override
        && whisper_model_is_complete(path)
    {
        return Some(ModelDiscovery {
            source: ModelSource::EnvOverride,
            path: path.to_path_buf(),
        });
    }

    let default_root = home_dir.join(".codescribe/models");
    let models_root = models_root
        .filter(|path| path.exists())
        .unwrap_or(&default_root);
    let user_fp16 = models_root.join(WHISPER_FP16_MODEL);
    if whisper_model_is_complete(&user_fp16) {
        return Some(ModelDiscovery {
            source: ModelSource::ModelsDir,
            path: user_fp16,
        });
    }

    None
}

pub fn model_discovery_hint(home_dir: &Path) -> String {
    let default_root = home_dir.join(".codescribe/models");
    let models_root = std::env::var("CODESCRIBE_MODELS_DIR")
        .ok()
        .map(|value| expand_models_root(home_dir, &value))
        .filter(|path| path.exists())
        .unwrap_or(default_root);
    format!(
        "The production resolver found no valid fp16 Whisper model in CODESCRIBE_MODEL_PATH, {root}/{fp16}, or a supported Hugging Face cache snapshot. The bundle must have parseable config and tokenizer files, the pinned mel_filters.npz checksum, structurally valid F16/F32 safetensors, and no quantization declaration.",
        root = models_root.display(),
        fp16 = WHISPER_FP16_MODEL
    )
}

pub fn normalize_transcript(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
