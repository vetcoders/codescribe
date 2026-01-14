//!
//! Build script for CodeScribe
//! Exports embedded model data and configuration.
//! Generates embedded_model_data.rs in OUT_DIR for release builds.
//!
//! Release builds REQUIRE the model by default.
//! Set CODESCRIBE_NO_EMBED=1 to build without embedding (for dev/CI).
//!
//! Created by M&K (c)2026 VetCoders

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Default model to embed
const MODEL_NAME: &str = "whisper-large-v3-turbo-mlx-q8";

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=models/{}", MODEL_NAME);

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

        let model_path = Path::new(&manifest_dir).join("models").join(MODEL_NAME);
        let out_dir = env::var("OUT_DIR").unwrap();
        let dest_path = Path::new(&out_dir).join("embedded_model_data.rs");
        let model_exists = model_path.join("tokenizer.json").exists();

        if is_release && model_exists {
            // Release + model found → embed it
            println!("cargo:warning=Embedding model from: {}", model_path.display());
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
            println!("cargo:rustc-env=CODESCRIBE_MODEL_DIR={}", model_path.display());
        } else if is_release && !model_exists && !no_embed {
            // Release + no model + no opt-out → HARD FAIL
            eprintln!();
            eprintln!("═══════════════════════════════════════════════════════════════");
            eprintln!("  ERROR: Whisper model not found for embedding!");
            eprintln!("═══════════════════════════════════════════════════════════════");
            eprintln!("  Expected: {}", model_path.display());
            eprintln!();
            eprintln!("  Solutions:");
            eprintln!("    1. Download model:  make download-model");
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
