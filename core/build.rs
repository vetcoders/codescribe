//!
//! Build script for CodeScribe
//! Exports embedded model data and configuration.
//! Generates embedded_tts_data.rs / embedded_embedder_data.rs / embedded_vad_data.rs in OUT_DIR.
//!
//! Whisper embedding is OPT-IN via CODESCRIBE_EMBED_WHISPER=1 (distribution builds).
//! Default builds resolve Whisper from the HF cache at runtime
//! (`resolve_runtime_whisper_model_path`) — the model is held in memory for the
//! session anyway, so baking ~1GB into every artifact only multiplied target/
//! into tens of GB for zero runtime win (2026-06-10 policy, operator-decided).
//! Release builds still embed Silero VAD + MiniLM embedder by default.
//! Opt-out of all optional embedding with CODESCRIBE_NO_EMBED=1 (except Silero).
//! TTS requires opt-in via CODESCRIBE_EMBED_TTS.
//!
//! ⚠ Embedded Whisper materially increases artifact size.
//!   TTS can still increase artifact size significantly — test before shipping!

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

/// Default embedder model — MiniLM multilingual (~224MB fp16, always embedded like Silero)
/// Override with CODESCRIBE_EMBEDDER_REPO for alternative models
const DEFAULT_EMBEDDER_MODEL_NAME: &str = "minilm-l12-v2";
const DEFAULT_EMBEDDER_REPO: &str = "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2";

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBED_MODEL");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_MODEL_PATH");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_NO_EMBED");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBED_WHISPER");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBED_TTS");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_TTS_PATH");
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

        let out_dir = env::var("OUT_DIR").unwrap();
        let embed_model = env::var("CODESCRIBE_EMBED_MODEL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL_NAME.to_string());
        let model_path =
            resolve_whisper_embed_model_path(&manifest_dir, &embed_model, DEFAULT_WHISPER_REPO);
        let weights_path = if model_path.join("weights.safetensors").exists() {
            model_path.join("weights.safetensors")
        } else {
            model_path.join("model.safetensors")
        };
        let model_exists = model_path.join("config.json").exists()
            && model_path.join("tokenizer.json").exists()
            && model_path.join("mel_filters.npz").exists()
            && weights_path.exists();
        if model_exists {
            println!(
                "cargo:rerun-if-changed={}",
                model_path.join("config.json").display()
            );
            println!(
                "cargo:rerun-if-changed={}",
                model_path.join("tokenizer.json").display()
            );
            println!(
                "cargo:rerun-if-changed={}",
                model_path.join("mel_filters.npz").display()
            );
            println!("cargo:rerun-if-changed={}", weights_path.display());
        }

        // Whisper model embedding (OPT-IN: distribution builds only).
        let embed_whisper_requested = env_flag("CODESCRIBE_EMBED_WHISPER", false);
        let whisper_dest_path = Path::new(&out_dir).join("embedded_model_data.rs");
        let whisper_embedded = embed_whisper_requested && !no_embed && model_exists;
        if whisper_embedded {
            println!(
                "cargo:warning=Embedding Whisper model from: {}",
                model_path.display()
            );
            let whisper_content = format!(
                r#"
                pub static CONFIG: &[u8] = include_bytes!(r"{}");
                pub static TOKENIZER: &[u8] = include_bytes!(r"{}");
                pub static MEL_FILTERS: &[u8] = include_bytes!(r"{}");
                pub static WEIGHTS: &[u8] = include_bytes!(r"{}");
                "#,
                model_path.join("config.json").display(),
                model_path.join("tokenizer.json").display(),
                model_path.join("mel_filters.npz").display(),
                weights_path.display(),
            );
            fs::write(&whisper_dest_path, whisper_content)
                .expect("Failed to write embedded_model_data.rs");
            println!("cargo:rustc-cfg=embed_model");
        } else if embed_whisper_requested && !no_embed && !model_exists {
            println!(
                "cargo:warning=Whisper model not found for embedding: {}",
                model_path.display()
            );
            println!(
                "cargo:warning=Download with: hf download {}",
                DEFAULT_WHISPER_REPO
            );
            println!("cargo:warning=Falling back to runtime Whisper lookup for this build");
        }

        // TTS model embedding (optional, via CODESCRIBE_EMBED_TTS=1)
        let embed_tts = env_flag("CODESCRIBE_EMBED_TTS", false) && !no_embed;
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

        // MiniLM embedder — always embedded (like Silero), ~224MB fp16
        // Skip only with CODESCRIBE_NO_EMBED=1
        let embedder_repo = env::var("CODESCRIBE_EMBEDDER_REPO")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_EMBEDDER_REPO.to_string());
        let embedder_model_path =
            resolve_embedder_model_path(&manifest_dir, DEFAULT_EMBEDDER_MODEL_NAME, &embedder_repo);
        let embedder_dest_path = Path::new(&out_dir).join("embedded_embedder_data.rs");
        let embedder_model_exists = embedder_model_path.join("config.json").exists()
            && embedder_model_path.join("tokenizer.json").exists()
            && embedder_model_path.join("model.safetensors").exists();

        if !no_embed && embedder_model_exists {
            println!(
                "cargo:warning=Embedding MiniLM model from: {}",
                embedder_model_path.display()
            );
            let embedder_content = format!(
                r#"
                pub static CONFIG: &[u8] = include_bytes!(r"{}");
                pub static TOKENIZER: &[u8] = include_bytes!(r"{}");
                pub static WEIGHTS: &[u8] = include_bytes!(r"{}");
                "#,
                embedder_model_path.join("config.json").display(),
                embedder_model_path.join("tokenizer.json").display(),
                embedder_model_path.join("model.safetensors").display(),
            );
            fs::write(&embedder_dest_path, embedder_content)
                .expect("Failed to write embedded_embedder_data.rs");
            println!("cargo:rustc-cfg=embed_embedder");
        } else if !no_embed && !embedder_model_exists {
            println!(
                "cargo:warning=Embedder model not found at: {}",
                embedder_model_path.display()
            );
            println!(
                "cargo:warning=Download with: huggingface-cli download {}",
                embedder_repo
            );
        }

        // Silero VAD — always embedded from repo (2.3MB, non-negotiable)
        let silero_path = Path::new(&manifest_dir)
            .parent()
            .unwrap_or(Path::new(&manifest_dir))
            .join("models")
            .join("silero_vad.onnx");
        let silero_dest_path = Path::new(&out_dir).join("embedded_vad_data.rs");
        println!("cargo:rerun-if-changed={}", silero_path.display());
        if silero_path.exists() {
            let silero_content = format!(
                r#"
                pub static MODEL: &[u8] = include_bytes!(r"{}");
                "#,
                silero_path.display(),
            );
            fs::write(&silero_dest_path, silero_content)
                .expect("Failed to write embedded_vad_data.rs");
            println!("cargo:rustc-cfg=embed_vad");
        } else {
            panic!(
                "Silero VAD model missing from repo: {}\nThis file must be committed to the repository.",
                silero_path.display()
            );
        }

        if is_release && whisper_embedded {
            println!("cargo:warning=Whisper build policy: embedded by default");
        }
        println!("cargo:rustc-env=CODESCRIBE_MODEL_DIR=");

        // Build-context detection: qube-* binaries are built into target-noembed/
        // (see Makefile `release-qube`) and never use Whisper/Embedder at runtime.
        // CODESCRIBE_NO_EMBED=1 has two distinct meanings:
        //   (a) operator install via `make install-no-embed` (codescribe binary, runtime load from HF cache)
        //   (b) build infra signal that this binary doesn't need STT models (qube-daemon, qube-report)
        // OUT_DIR is the only signal that disambiguates them.
        let qube_context = out_dir.contains("target-noembed");
        let context_label = if qube_context {
            "qube-tools"
        } else if no_embed {
            "codescribe (no-embed dev install)"
        } else {
            "codescribe"
        };

        let whisper_summary = if whisper_embedded {
            "embedded"
        } else if qube_context {
            "not_used"
        } else if embed_whisper_requested && !no_embed {
            // Embed was explicitly requested but the snapshot is incomplete.
            "missing_at_build_time"
        } else {
            // Default policy (2026-06-10): resolve from the HF cache at runtime.
            "runtime_load_from_cache"
        };
        let embedder_summary = if qube_context {
            "not_used"
        } else if !no_embed && embedder_model_exists {
            "embedded"
        } else if no_embed {
            "runtime_load_from_cache"
        } else {
            "missing_at_build_time"
        };
        let tts_summary = if qube_context {
            "not_used"
        } else if embed_tts && tts_model_exists && mimi_weights_path.exists() {
            "embedded"
        } else if embed_tts {
            "missing_at_build_time"
        } else {
            "disabled"
        };
        println!(
            "cargo:warning=Embedded models for {}: Whisper={}; Silero=embedded; Embedder={}; TTS={}",
            context_label, whisper_summary, embedder_summary, tts_summary
        );
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
    // CODESCRIBE_MODEL_PATH takes priority — explicit user override
    if let Ok(model_path) = env::var("CODESCRIBE_MODEL_PATH") {
        let p = PathBuf::from(model_path.trim());
        if p.join("config.json").exists() {
            return p;
        }
    }
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

fn resolve_embedder_model_path(manifest_dir: &str, embed_model: &str, repo: &str) -> PathBuf {
    if let Some(snapshot) = find_hf_snapshot(repo) {
        return snapshot;
    }
    resolve_embed_model_path(manifest_dir, embed_model)
}

fn hf_cache_bases() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(path) = env::var("CODESCRIBE_HF_CACHE") {
        out.push(PathBuf::from(path));
    }
    if let Ok(path) = env::var("HUGGINGFACE_HUB_CACHE") {
        out.push(PathBuf::from(path));
    }
    if let Ok(path) = env::var("HF_HUB_CACHE") {
        out.push(PathBuf::from(path));
    }
    if let Ok(path) = env::var("HF_HOME") {
        out.push(PathBuf::from(path).join("hub"));
    }
    if let Some(home) = dirs::home_dir().map(|h| h.join(".cache").join("huggingface").join("hub")) {
        out.push(home);
    }
    if let Some(home) = dirs::home_dir().map(|h| h.join(".codescribe").join("embeddings")) {
        out.push(home.clone());
        out.push(home.join("hub"));
    }
    out.sort();
    out.dedup();
    out
}

fn find_hf_snapshot(repo: &str) -> Option<PathBuf> {
    for base in hf_cache_bases() {
        if let Some(snapshot) = find_hf_snapshot_in_base(&base, repo) {
            return Some(snapshot);
        }
    }
    None
}

fn find_hf_snapshot_in_base(base: &PathBuf, repo: &str) -> Option<PathBuf> {
    let repo_dir = base.join(format!("models--{}", repo.replace('/', "--")));
    let snapshots_dir = repo_dir.join("snapshots");

    let snapshots_dir = if snapshots_dir.exists() {
        snapshots_dir
    } else {
        let target = repo.to_ascii_lowercase();
        let mut matched: Option<PathBuf> = None;
        if let Ok(entries) = fs::read_dir(base) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with("models--") {
                    continue;
                }
                let repo_id = name
                    .strip_prefix("models--")
                    .unwrap_or("")
                    .replace("--", "/");
                if repo_id.to_ascii_lowercase() == target {
                    matched = Some(entry.path().join("snapshots"));
                    break;
                }
            }
        }
        matched?
    };

    let entries = fs::read_dir(&snapshots_dir).ok()?;

    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
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
