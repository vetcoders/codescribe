//! Shared helpers for STT E2E matrix tests.
//!
//! Goals:
//! - Keep heavy tests explicitly opt-in (`CODESCRIBE_E2E_*`).
//! - Keep deterministic checks always-on.
//! - Reuse one model discovery strategy across E2E suites.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const STT_OPT_IN_ENV: &str = "CODESCRIBE_E2E_STT";
pub const ROUNDTRIP_OPT_IN_ENV: &str = "CODESCRIBE_E2E_ROUNDTRIP";

pub const WHISPER_TURBO_MODEL: &str = "whisper-large-v3-turbo-mlx-q8";
pub const WHISPER_LARGE_MODEL: &str = "whisper-large-v3-mlx-q8";

const HF_TURBO_REPO_DIRS: &[&str] = &[
    "models--LibraxisAI--whisper-large-v3-turbo-mlx-q8",
    "models--libraxisai--whisper-large-v3-turbo-mlx-q8",
];
const HF_LARGE_REPO_DIRS: &[&str] = &[
    "models--LibraxisAI--whisper-large-v3-mlx-q8",
    "models--libraxisai--whisper-large-v3-mlx-q8",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSource {
    EnvOverride,
    UserTurbo,
    UserLarge,
    HfTurboSnapshot,
    HfLargeSnapshot,
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
    let has_weights =
        path.join("weights.safetensors").exists() || path.join("model.safetensors").exists();
    path.join("config.json").exists()
        && path.join("tokenizer.json").exists()
        && path.join("mel_filters.npz").exists()
        && has_weights
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

pub fn default_hf_cache_bases(home_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();

    if let Ok(path) = std::env::var("CODESCRIBE_HF_CACHE") {
        out.push(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var("HUGGINGFACE_HUB_CACHE") {
        out.push(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var("HF_HUB_CACHE") {
        out.push(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var("HF_HOME") {
        out.push(PathBuf::from(path).join("hub"));
    }

    out.push(home_dir.join(".cache/huggingface/hub"));
    out.sort();
    out.dedup();
    out
}

pub fn discover_local_whisper_model() -> Option<ModelDiscovery> {
    let home_dir = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let env_override = std::env::var("CODESCRIBE_MODEL_PATH")
        .ok()
        .map(PathBuf::from);
    let hf_bases = default_hf_cache_bases(&home_dir);
    discover_local_whisper_model_for(&home_dir, env_override.as_deref(), &hf_bases)
}

pub fn discover_local_whisper_model_for(
    home_dir: &Path,
    env_override: Option<&Path>,
    hf_cache_bases: &[PathBuf],
) -> Option<ModelDiscovery> {
    if let Some(path) = env_override
        && whisper_model_is_complete(path)
    {
        return Some(ModelDiscovery {
            source: ModelSource::EnvOverride,
            path: path.to_path_buf(),
        });
    }

    let user_turbo = home_dir
        .join(".codescribe/models")
        .join(WHISPER_TURBO_MODEL);
    if whisper_model_is_complete(&user_turbo) {
        return Some(ModelDiscovery {
            source: ModelSource::UserTurbo,
            path: user_turbo,
        });
    }

    let user_large = home_dir
        .join(".codescribe/models")
        .join(WHISPER_LARGE_MODEL);
    if whisper_model_is_complete(&user_large) {
        return Some(ModelDiscovery {
            source: ModelSource::UserLarge,
            path: user_large,
        });
    }

    if let Some(path) = find_latest_hf_snapshot(hf_cache_bases, HF_TURBO_REPO_DIRS) {
        return Some(ModelDiscovery {
            source: ModelSource::HfTurboSnapshot,
            path,
        });
    }

    if let Some(path) = find_latest_hf_snapshot(hf_cache_bases, HF_LARGE_REPO_DIRS) {
        return Some(ModelDiscovery {
            source: ModelSource::HfLargeSnapshot,
            path,
        });
    }

    None
}

pub fn model_discovery_hint(home_dir: &Path) -> String {
    format!(
        "Looked for complete Whisper model in CODESCRIBE_MODEL_PATH, {home}/.codescribe/models/{turbo}, {home}/.codescribe/models/{large}, and HF cache snapshots. Required files: config.json, tokenizer.json, mel_filters.npz, weights.safetensors or model.safetensors.",
        home = home_dir.display(),
        turbo = WHISPER_TURBO_MODEL,
        large = WHISPER_LARGE_MODEL
    )
}

pub fn normalize_transcript(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_latest_hf_snapshot(hf_cache_bases: &[PathBuf], repo_dir_names: &[&str]) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;

    for base in hf_cache_bases {
        for repo in repo_dir_names {
            let snapshots = base.join(repo).join("snapshots");
            let entries = match std::fs::read_dir(&snapshots) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() || !whisper_model_is_complete(&path) {
                    continue;
                }

                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);

                match &best {
                    Some((best_time, _)) if *best_time >= modified => {}
                    _ => best = Some((modified, path)),
                }
            }
        }
    }

    best.map(|(_, path)| path)
}
