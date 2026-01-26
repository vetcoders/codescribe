//!
//! Build script for CodeScribe
//! Exports embedded model data and configuration.
//! Generates embedded_model_data.rs and embedded_tts_data.rs in OUT_DIR for release builds.
//!
//! Release builds REQUIRE the Whisper model by default.
//! Set CODESCRIBE_NO_EMBED=1 to build without embedding (for dev/CI).
//! TTS model embedding is optional via CODESCRIBE_EMBED_TTS=1.
//! E5 embedder embedding is ON by default in release builds.
//! Set CODESCRIBE_EMBED_E5=0 to disable E5 embedding.
//!
//! Created by M&K (c)2026 VetCoders

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Default Whisper model to embed
const DEFAULT_MODEL_NAME: &str = "whisper-large-v3-turbo-mlx-q8";
const DEFAULT_WHISPER_REPO: &str = "LibraxisAI/whisper-large-v3-turbo-mlx-q8";

/// Default TTS model to embed
const DEFAULT_TTS_MODEL_NAME: &str = "csm-1b";
const DEFAULT_TTS_REPO: &str = "sesame/csm-1b";
const DEFAULT_MIMI_REPO: &str = "kyutai/mimi";

/// Default E5 embedder model to embed
const DEFAULT_E5_MODEL_NAME: &str = "e5-large";
const DEFAULT_E5_REPO: &str = "intfloat/multilingual-e5-large";

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBED_MODEL");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_NO_EMBED");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBED_TTS");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_TTS_PATH");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBED_E5");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBEDDER_REPO");

    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let is_release = profile == "release";
    let no_embed = env::var("CODESCRIBE_NO_EMBED").is_ok();

    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let codescribe_dir = dirs::home_dir()
            .map(|h| h.join(".codescribe"))
            .unwrap_or_else(|| PathBuf::from("/tmp/.codescribe"));

        if is_release {
            let _ = fs::create_dir_all(&codescribe_dir);
            let repo_path_file = codescribe_dir.join("repo_path");
            let _ = fs::write(&repo_path_file, &manifest_dir);
        }

        let embed_model = env::var("CODESCRIBE_EMBED_MODEL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL_NAME.to_string());
        let model_path =
            resolve_whisper_embed_model_path(&manifest_dir, &embed_model, DEFAULT_WHISPER_REPO);
        println!("cargo:rerun-if-changed={}", model_path.display());
        let out_dir = env::var("OUT_DIR").unwrap();
        let dest_path = Path::new(&out_dir).join("embedded_model_data.rs");
        let model_exists = model_path.join("tokenizer.json").exists();

        // TTS model embedding (optional, via CODESCRIBE_EMBED_TTS=1)
        let embed_tts = env::var("CODESCRIBE_EMBED_TTS").is_ok();
        let tts_model_path =
            resolve_tts_embed_model_path(&manifest_dir, DEFAULT_TTS_MODEL_NAME, DEFAULT_TTS_REPO);
        let tts_dest_path = Path::new(&out_dir).join("embedded_tts_data.rs");
        let tts_model_exists = tts_model_path.join("config.json").exists();
        let mimi_path_from_cache =
            find_hf_snapshot(DEFAULT_MIMI_REPO).map(|p| p.join("model.safetensors"));
        let mimi_weights_path = if tts_model_path.join("mimi.safetensors").exists() {
            tts_model_path.join("mimi.safetensors")
        } else {
            mimi_path_from_cache.unwrap_or_else(|| tts_model_path.join("mimi.safetensors"))
        };

        if embed_tts && tts_model_exists && mimi_weights_path.exists() {
            println!(
                "cargo:warning=Embedding TTS model from: {}",
                tts_model_path.display()
            );
            let tts_content = format!(
                r#"
                pub static CONFIG: &[u8] = include_bytes!(r"{}");
                pub static TOKENIZER: &[u8] = include_bytes!(r"{}");
                pub static WEIGHTS: &[u8] = include_bytes!(r"{}");
                pub static MIMI_CONFIG: &[u8] = &[]; // Mimi uses factory config
                pub static MIMI_WEIGHTS: &[u8] = include_bytes!(r"{}");
                pub static VOICE_TOKENS: &[u8] = &[]; // Optional voice tokens
                "#,
                tts_model_path.join("config.json").display(),
                tts_model_path.join("tokenizer.json").display(),
                tts_model_path.join("model.safetensors").display(),
                mimi_weights_path.display(),
            );
            fs::write(&tts_dest_path, tts_content).expect("Failed to write embedded_tts_data.rs");
            println!("cargo:rustc-cfg=embed_tts");
        } else if embed_tts && (!tts_model_exists || !mimi_weights_path.exists()) {
            println!(
                "cargo:warning=CODESCRIBE_EMBED_TTS set but TTS model not found at: {}",
                tts_model_path.display()
            );
            println!(
                "cargo:warning=Download with: hf download {}",
                DEFAULT_TTS_REPO
            );
            println!(
                "cargo:warning=Download Mimi with: hf download {}",
                DEFAULT_MIMI_REPO
            );
        }

        // E5 embedder embedding (default ON in release; disable with CODESCRIBE_EMBED_E5=0)
        let embed_e5 = env_flag("CODESCRIBE_EMBED_E5", is_release);
        let e5_repo = env::var("CODESCRIBE_EMBEDDER_REPO")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_E5_REPO.to_string());
        let e5_model_path =
            resolve_e5_embed_model_path(&manifest_dir, DEFAULT_E5_MODEL_NAME, &e5_repo);
        let e5_dest_path = Path::new(&out_dir).join("embedded_e5_data.rs");
        let e5_model_exists = e5_model_path.join("config.json").exists()
            && e5_model_path.join("tokenizer.json").exists()
            && e5_model_path.join("model.safetensors").exists();

        if embed_e5 && e5_model_exists {
            println!(
                "cargo:warning=Embedding E5 model from: {}",
                e5_model_path.display()
            );
            let e5_content = format!(
                r#"
                pub static CONFIG: &[u8] = include_bytes!(r"{}");
                pub static TOKENIZER: &[u8] = include_bytes!(r"{}");
                pub static WEIGHTS: &[u8] = include_bytes!(r"{}");
                "#,
                e5_model_path.join("config.json").display(),
                e5_model_path.join("tokenizer.json").display(),
                e5_model_path.join("model.safetensors").display(),
            );
            fs::write(&e5_dest_path, e5_content).expect("Failed to write embedded_e5_data.rs");
            println!("cargo:rustc-cfg=embed_e5");
        } else if embed_e5 && !e5_model_exists {
            if is_release {
                eprintln!();
                eprintln!("═══════════════════════════════════════════════════════════════");
                eprintln!("  ERROR: E5 model not found for embedding!");
                eprintln!("═══════════════════════════════════════════════════════════════");
                eprintln!("  Expected: {}", e5_model_path.display());
                eprintln!();
                eprintln!("  Solutions:");
                eprintln!("    1. Download model:  hf download {}", e5_repo);
                eprintln!("    2. Skip embedding:  CODESCRIBE_EMBED_E5=0 cargo build --release");
                eprintln!("═══════════════════════════════════════════════════════════════");
                eprintln!();
                std::process::exit(1);
            } else {
                println!(
                    "cargo:warning=E5 model not found at: {}",
                    e5_model_path.display()
                );
                println!("cargo:warning=Download with: hf download {}", e5_repo);
            }
        } else if !embed_e5 && is_release {
            println!("cargo:warning=CODESCRIBE_EMBED_E5=0 - skipping E5 embedding");
        }

        if is_release && model_exists {
            // Release + model found → embed it
            println!(
                "cargo:warning=Embedding model from: {}",
                model_path.display()
            );
            let content = format!(
                r#"
                pub static CONFIG: &[u8] = include_bytes!(r"{}");
                pub static TOKENIZER: &[u8] = include_bytes!(r"{}");
                pub static MEL_FILTERS: &[u8] = include_bytes!(r"{}");
                pub static WEIGHTS: &[u8] = include_bytes!(r"{}");
                "#,
                model_path.join("config.json").display(),
                model_path.join("tokenizer.json").display(),
                model_path.join("mel_filters.npz").display(),
                model_path.join("weights.safetensors").display()
            );
            fs::write(&dest_path, content).expect("Failed to write embedded_model_data.rs");
            println!("cargo:rustc-cfg=embed_model");
            println!(
                "cargo:rustc-env=CODESCRIBE_MODEL_DIR={}",
                model_path.display()
            );
        } else if is_release && !model_exists && !no_embed {
            // Release + no model + no opt-out → HARD FAIL
            eprintln!();
            eprintln!("═══════════════════════════════════════════════════════════════");
            eprintln!("  ERROR: Whisper model not found for embedding!");
            eprintln!("═══════════════════════════════════════════════════════════════");
            eprintln!("  Expected: {}", model_path.display());
            eprintln!();
            eprintln!("  Solutions:");
            eprintln!(
                "    1. Download model:  hf download {}",
                DEFAULT_WHISPER_REPO
            );
            eprintln!("    2. Skip embedding:  CODESCRIBE_NO_EMBED=1 cargo build --release");
            eprintln!();
            eprintln!("  Note: Without embedded model, set CODESCRIBE_MODEL_PATH at runtime.");
            eprintln!("═══════════════════════════════════════════════════════════════");
            eprintln!();
            std::process::exit(1);
        } else {
            // Debug build OR explicit no-embed → skip embedding
            if is_release && no_embed {
                println!("cargo:warning=CODESCRIBE_NO_EMBED set - skipping model embedding");
                println!("cargo:warning=Binary will require CODESCRIBE_MODEL_PATH at runtime");
            }
            println!("cargo:rustc-env=CODESCRIBE_MODEL_DIR=");
        }
    }
}

fn resolve_embed_model_path(manifest_dir: &str, embed_model: &str) -> PathBuf {
    let candidate = PathBuf::from(embed_model);
    if candidate.is_absolute() {
        return candidate;
    }

    if candidate.components().count() > 1 {
        return Path::new(manifest_dir).join(candidate);
    }

    Path::new(manifest_dir).join("models").join(embed_model)
}

fn resolve_whisper_embed_model_path(
    manifest_dir: &str,
    embed_model: &str,
    default_repo: &str,
) -> PathBuf {
    if embed_model.contains('/') {
        if let Some(snapshot) = find_hf_snapshot(embed_model) {
            return snapshot;
        }
    } else if embed_model == DEFAULT_MODEL_NAME
        && let Some(snapshot) = find_hf_snapshot(default_repo)
    {
        return snapshot;
    }
    resolve_embed_model_path(manifest_dir, embed_model)
}

fn resolve_tts_embed_model_path(
    manifest_dir: &str,
    embed_model: &str,
    default_repo: &str,
) -> PathBuf {
    if embed_model.contains('/') {
        if let Some(snapshot) = find_hf_snapshot(embed_model) {
            return snapshot;
        }
    } else if embed_model == DEFAULT_TTS_MODEL_NAME
        && let Some(snapshot) = find_hf_snapshot(default_repo)
    {
        return snapshot;
    }
    resolve_embed_model_path(manifest_dir, embed_model)
}

fn resolve_e5_embed_model_path(manifest_dir: &str, embed_model: &str, repo: &str) -> PathBuf {
    if let Some(snapshot) = find_hf_snapshot(repo) {
        return snapshot;
    }
    resolve_embed_model_path(manifest_dir, embed_model)
}

fn hf_cache_base() -> Option<PathBuf> {
    if let Ok(path) = env::var("HUGGINGFACE_HUB_CACHE") {
        return Some(PathBuf::from(path));
    }
    if let Ok(path) = env::var("HF_HUB_CACHE") {
        return Some(PathBuf::from(path));
    }
    if let Ok(path) = env::var("HF_HOME") {
        return Some(PathBuf::from(path).join("hub"));
    }
    dirs::home_dir().map(|h| h.join(".cache").join("huggingface").join("hub"))
}

fn find_hf_snapshot(repo: &str) -> Option<PathBuf> {
    let base = hf_cache_base()?;
    let repo_dir = base.join(format!("models--{}", repo.replace('/', "--")));
    let snapshots_dir = repo_dir.join("snapshots");
    let entries = fs::read_dir(&snapshots_dir).ok()?;

    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let has_files = path.join("config.json").exists()
            && path.join("tokenizer.json").exists()
            && path.join("model.safetensors").exists();
        if !has_files {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        match &best {
            Some((best_time, _)) if *best_time >= modified => {}
            _ => best = Some((modified, path)),
        }
    }

    best.map(|(_, p)| p)
}

fn env_flag(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return default;
            }
            let v = trimmed.to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "no")
        }
        Err(_) => default,
    }
}
