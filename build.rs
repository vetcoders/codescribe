//!
//! Build script for CodeScribe
//! Exports embedded model data and configuration.
//! Generates embedded_model_data.rs in OUT_DIR for release builds.
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
    let should_try_embed = profile == "release";

    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let codescribe_dir = dirs::home_dir()
            .map(|h| h.join(".codescribe"))
            .unwrap_or_else(|| PathBuf::from("/tmp/.codescribe"));

        if profile == "release" {
            let _ = fs::create_dir_all(&codescribe_dir);
            let repo_path_file = codescribe_dir.join("repo_path");
            let _ = fs::write(&repo_path_file, &manifest_dir);
        }

        let model_path = Path::new(&manifest_dir).join("models").join(MODEL_NAME);
        let out_dir = env::var("OUT_DIR").unwrap();
        let dest_path = Path::new(&out_dir).join("embedded_model_data.rs");

        if should_try_embed && model_path.join("tokenizer.json").exists() {
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
        } else {
            if should_try_embed {
                 println!("cargo:warning=Model not found for embedding at: {}", model_path.display());
                 println!("cargo:warning=Run: ./scripts/download-model.sh");
            }
            println!("cargo:rustc-env=CODESCRIBE_MODEL_DIR=");
        }
    }
}
