//!
//! Build script for Codescribe
//! Exports embedded model data and configuration.
//! Generates embedded_tts_data.rs / embedded_embedder_data.rs / embedded_vad_data.rs in OUT_DIR.
//!
//! Whisper embedding is OPT-IN via CODESCRIBE_EMBED_WHISPER=1 (distribution builds).
//! Default builds resolve Whisper from the HF cache at runtime
//! (`resolve_runtime_whisper_model_path`) — the model is held in memory for the
//! session anyway, so baking ~1GB into every artifact only multiplied target/
//! into tens of GB for zero runtime win (2026-06-10 policy, operator-decided).
//! MiniLM follows the same runtime-load policy as Whisper: normal builds load
//! it from the app resource bundle or HF cache; only
//! `CODESCRIBE_EMBED_EMBEDDER=1` bakes it into the Rust artifact. Silero VAD
//! remains embedded because it is small and part of capture identity.
//! Opt-out of all optional embedding with CODESCRIBE_NO_EMBED=1 (except Silero).
//! TTS requires opt-in via CODESCRIBE_EMBED_TTS.
//!
//! ⚠ Embedded Whisper materially increases artifact size.
//!   TTS can still increase artifact size significantly — test before shipping!

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The license key contract, included by path so the build script and the
/// crate it builds share one definition of the dev key and its fingerprint
/// instead of two copies that can drift apart.
#[path = "licensing/key_contract.rs"]
mod license_key_contract;
use license_key_contract::{
    DEV_LICENSE_PUBLIC_KEY_FINGERPRINT, DEV_LICENSE_PUBLIC_KEY_HEX, LICENSE_PUBLIC_KEY_ENV,
};

/// Default Whisper model to embed
const DEFAULT_MODEL_NAME: &str = "whisper-large-v3-turbo";
/// Hugging Face repo id for the default Whisper snapshot (HF cache + download hints).
/// The repo ships only config + fp16 weights; `make download-model` composes
/// tokenizer.json + mel_filters.npz from the legacy q8 repo, and runtime keeps
/// a legacy fallback (see core/config/models.rs).
const DEFAULT_WHISPER_REPO: &str = "mlx-community/whisper-large-v3-turbo";

/// Default TTS model to embed
const DEFAULT_TTS_MODEL_NAME: &str = "csm-1b";
/// Hugging Face repo id for the default CSM TTS snapshot.
const DEFAULT_TTS_REPO: &str = "sesame/csm-1b";
/// Hugging Face repo id for Mimi codec weights used with TTS embedding.
const DEFAULT_MIMI_REPO: &str = "kyutai/mimi";

/// Default embedder model — MiniLM multilingual (~471MB fp32 weights on disk).
/// Override with CODESCRIBE_EMBEDDER_REPO for alternative models
const DEFAULT_EMBEDDER_MODEL_NAME: &str = "minilm-l12-v2";
/// Default sentence-transformers MiniLM repo resolved from bundle/cache at runtime.
const DEFAULT_EMBEDDER_REPO: &str = "sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2";
/// Env flag: local install path skips release license-key hardening in `main`.
const LOCAL_INSTALL_ENV: &str = "CODESCRIBE_LOCAL_INSTALL";

/// Resolve every optional model asset and emit the `embed_*` cfgs the crate
/// compiles against.
///
/// Each asset has its own policy, and they are not symmetrical: Silero VAD is
/// non-negotiable and its absence panics; MiniLM, Whisper and TTS are opt-in.
/// MiniLM and TTS degrade to a `cargo:warning` when requested but missing.
/// Whisper embed is fail-closed: `CODESCRIBE_EMBED_WHISPER=1` without a
/// complete snapshot must not produce a `_full` artifact that is actually slim.
fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBED_MODEL");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_MODEL_PATH");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_NO_EMBED");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBED_WHISPER");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBED_EMBEDDER");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBED_TTS");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_TTS_PATH");
    println!("cargo:rerun-if-env-changed=CODESCRIBE_EMBEDDER_REPO");
    println!("cargo:rerun-if-env-changed={LICENSE_PUBLIC_KEY_ENV}");
    println!("cargo:rerun-if-env-changed={LOCAL_INSTALL_ENV}");

    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let is_release = profile == "release";
    let is_local_install = env::var("CODESCRIBE_LOCAL_INSTALL")
        .ok()
        .is_some_and(|value| value == "1");
    let no_embed = env::var("CODESCRIBE_NO_EMBED").is_ok();

    configure_license_public_key(is_release && !is_local_install);

    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
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
                /// Embedded Whisper config.json bytes (opt-in fat SKU only).
                pub static CONFIG: &[u8] = include_bytes!(r"{}");
                /// Embedded Whisper tokenizer.json bytes.
                pub static TOKENIZER: &[u8] = include_bytes!(r"{}");
                /// Embedded Whisper mel filter bank (mel_filters.npz).
                pub static MEL_FILTERS: &[u8] = include_bytes!(r"{}");
                /// Embedded Whisper model weights (safetensors).
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
            panic!(
                "CODESCRIBE_EMBED_WHISPER=1 but no complete Whisper snapshot at {}. \
Need config.json + tokenizer.json + mel_filters.npz + weights/model.safetensors. \
The HF repo {} is weights-only; compose it with `make download-model` into \
~/.codescribe/models/{}, or set CODESCRIBE_MODEL_PATH to that directory.",
                model_path.display(),
                DEFAULT_WHISPER_REPO,
                DEFAULT_MODEL_NAME
            );
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
                /// Embedded TTS config.json bytes.
                pub static CONFIG: &[u8] = include_bytes!(r"{}");
                /// Embedded TTS tokenizer.json bytes.
                pub static TOKENIZER: &[u8] = include_bytes!(r"{}");
                /// Embedded TTS model.safetensors weights.
                pub static WEIGHTS: &[u8] = include_bytes!(r"{}");
                /// Placeholder Mimi config (factory defaults; not file-backed).
                pub static MIMI_CONFIG: &[u8] = &[]; // Mimi uses factory config
                /// Embedded Mimi codec weights for TTS decoding.
                pub static MIMI_WEIGHTS: &[u8] = include_bytes!(r"{}");
                /// Optional voice-token blob (empty when unused).
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

        // MiniLM embedder — runtime bundle/cache by default, matching Whisper.
        // Binary embedding is an explicit fat-SKU/debug request only.
        let embed_embedder_requested = env_flag("CODESCRIBE_EMBED_EMBEDDER", false);
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

        let embedder_embedded = embed_embedder_requested && !no_embed && embedder_model_exists;
        if embedder_embedded {
            println!(
                "cargo:warning=Embedding MiniLM model from: {}",
                embedder_model_path.display()
            );
            let embedder_content = format!(
                r#"
                /// Embedded MiniLM embedder config.json bytes.
                pub static CONFIG: &[u8] = include_bytes!(r"{}");
                /// Embedded MiniLM tokenizer.json bytes.
                pub static TOKENIZER: &[u8] = include_bytes!(r"{}");
                /// Embedded MiniLM model.safetensors weights.
                pub static WEIGHTS: &[u8] = include_bytes!(r"{}");
                "#,
                embedder_model_path.join("config.json").display(),
                embedder_model_path.join("tokenizer.json").display(),
                embedder_model_path.join("model.safetensors").display(),
            );
            fs::write(&embedder_dest_path, embedder_content)
                .expect("Failed to write embedded_embedder_data.rs");
            println!("cargo:rustc-cfg=embed_embedder");
        } else if embed_embedder_requested && !no_embed && !embedder_model_exists {
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
                /// Embedded Silero VAD ONNX model bytes (always required).
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
            println!(
                "cargo:warning=Whisper build policy: OPT-IN embed active (CODESCRIBE_EMBED_WHISPER=1) — fat SKU, not daily default"
            );
        } else if is_release {
            println!(
                "cargo:warning=Whisper build policy: runtime/cache (slim default); set CODESCRIBE_EMBED_WHISPER=1 only for fat SKU"
            );
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
        } else if embedder_embedded {
            "embedded"
        } else if embed_embedder_requested && !no_embed {
            "missing_at_build_time"
        } else {
            "runtime_load_from_bundle_or_cache"
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

/// Bake the license verification key into the binary, refusing to ship a
/// release that would verify against the development key.
///
/// Release builds must supply the key through the environment; three separate
/// conditions abort instead of degrading — the `licensing-dev` feature being
/// on, the env key being absent, and a supplied key whose fingerprint is the
/// known development one. A build that cannot verify licences honestly is
/// worse than no build, so each of these panics rather than warns.
fn configure_license_public_key(is_release: bool) {
    if is_release && env::var_os("CARGO_FEATURE_LICENSING_DEV").is_some() {
        panic!("licensing-dev is forbidden in release builds");
    }

    let configured = env::var(LICENSE_PUBLIC_KEY_ENV)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let key_hex = match configured {
        Some(value) => value,
        None if is_release => panic!(
            "{LICENSE_PUBLIC_KEY_ENV} is required for release builds; refusing to embed the development license key"
        ),
        None => DEV_LICENSE_PUBLIC_KEY_HEX.to_string(),
    };

    let key_bytes = decode_license_public_key(&key_hex);
    let fingerprint = format!("{:x}", Sha256::digest(key_bytes));
    if is_release && fingerprint == DEV_LICENSE_PUBLIC_KEY_FINGERPRINT {
        panic!(
            "{LICENSE_PUBLIC_KEY_ENV} has the forbidden development fingerprint {DEV_LICENSE_PUBLIC_KEY_FINGERPRINT}"
        );
    }

    println!("cargo:rustc-env=CODESCRIBE_LICENSE_PUBLIC_KEY_HEX={key_hex}");
    println!("cargo:rustc-env=CODESCRIBE_LICENSE_PUBLIC_KEY_FINGERPRINT={fingerprint}");
}

/// Decode a 64-character hex Ed25519 public key.
///
/// Asserts the shape before decoding: anything else — a UUID, a truncated
/// paste, a path — fails the build here rather than producing a binary whose
/// licence verification silently never matches.
fn decode_license_public_key(value: &str) -> [u8; 32] {
    assert!(
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{LICENSE_PUBLIC_KEY_ENV} must contain exactly 64 hexadecimal characters"
    );
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&value[start..start + 2], 16)
            .expect("validated license public key hex");
    }
    bytes
}

/// Last-resort path resolution for a model, once no HF snapshot matched.
///
/// An absolute path is taken verbatim, a multi-component relative path is
/// resolved against the manifest dir, and a bare name is looked up under
/// `<manifest>/models/`.
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

/// True when a directory can be baked into the fat SKU.
///
/// The default HF Whisper repo is weights-only. `make download-model` composes
/// tokenizer + mel into `~/.codescribe/models/<name>`. Incomplete snapshots
/// must not win over that composed tree.
fn whisper_dir_complete(path: &Path) -> bool {
    let weights = if path.join("weights.safetensors").exists() {
        path.join("weights.safetensors")
    } else {
        path.join("model.safetensors")
    };
    path.join("config.json").exists()
        && path.join("tokenizer.json").exists()
        && path.join("mel_filters.npz").exists()
        && weights.exists()
}

/// Locate the Whisper snapshot to embed.
///
/// `CODESCRIBE_MODEL_PATH` wins when it is a complete snapshot. The composed
/// `~/.codescribe/models` tree is next — that is what `make download-model`
/// writes. An HF cache hit is used only when it already has tokenizer + mel.
fn resolve_whisper_embed_model_path(
    manifest_dir: &str,
    embed_model: &str,
    default_repo: &str,
) -> PathBuf {
    if let Ok(model_path) = env::var("CODESCRIBE_MODEL_PATH") {
        let p = PathBuf::from(model_path.trim());
        if whisper_dir_complete(&p) {
            return p;
        }
    }
    if let Some(home) = dirs::home_dir() {
        let composed = home.join(".codescribe").join("models").join(embed_model);
        if whisper_dir_complete(&composed) {
            return composed;
        }
        if embed_model == DEFAULT_MODEL_NAME {
            let default_composed = home
                .join(".codescribe")
                .join("models")
                .join(DEFAULT_MODEL_NAME);
            if whisper_dir_complete(&default_composed) {
                return default_composed;
            }
        }
    }
    if embed_model.contains('/')
        && let Some(snapshot) = find_hf_snapshot(embed_model)
        && whisper_dir_complete(&snapshot)
    {
        return snapshot;
    } else if embed_model == DEFAULT_MODEL_NAME
        && let Some(snapshot) = find_hf_snapshot(default_repo)
        && whisper_dir_complete(&snapshot)
    {
        return snapshot;
    }
    resolve_embed_model_path(manifest_dir, embed_model)
}

/// Locate the TTS snapshot to embed. Same cache-then-path resolution as
/// Whisper, minus the `CODESCRIBE_MODEL_PATH` override, which is Whisper's
/// alone.
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

/// Locate the embedder snapshot. The repo is always consulted first — unlike
/// Whisper and TTS there is no bare-name special case, because the embedder
/// repo is either the default or an explicit `CODESCRIBE_EMBEDDER_REPO`.
fn resolve_embedder_model_path(manifest_dir: &str, embed_model: &str, repo: &str) -> PathBuf {
    if let Some(snapshot) = find_hf_snapshot(repo) {
        return snapshot;
    }
    resolve_embed_model_path(manifest_dir, embed_model)
}

/// Every directory that may hold a Hugging Face cache, deduplicated.
///
/// Covers the project override, the three standard HF env vars, the default
/// user cache, and Codescribe's own embeddings dir. Sorted and deduped because
/// several of these routinely point at the same place.
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

/// First snapshot of `repo` found across the candidate cache bases.
fn find_hf_snapshot(repo: &str) -> Option<PathBuf> {
    for base in hf_cache_bases() {
        if let Some(snapshot) = find_hf_snapshot_in_base(&base, repo) {
            return Some(snapshot);
        }
    }
    None
}

/// Newest snapshot of `repo` under one cache base.
///
/// The `models--owner--name` directory is tried first; failing that, the base
/// is scanned case-insensitively, because HF repo ids differ in case between
/// what a caller writes and what the cache recorded. Among the snapshots, the
/// most recently modified wins — that is the one a `hf download` just wrote.
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

/// Read a boolean build flag from the environment.
///
/// Anything set counts as true except the explicit negatives (`0`, `false`,
/// `off`, `no`); an unset *or* whitespace-only value falls back to `default`,
/// so an empty CI variable does not silently mean "on".
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
