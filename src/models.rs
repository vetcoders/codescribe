//! Model management for Whisper models.
//!
//! This module provides utilities for listing available models.
//! For actual transcription, use `whisper::singleton` which provides
//! a pre-loaded engine.
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Default bundled model name
pub const DEFAULT_MODEL: &str = "whisper-large-v3-turbo-mlx-q8";

pub struct ModelManager {
    models_dir: PathBuf,
}

impl ModelManager {
    /// Create a new ModelManager.
    ///
    /// Resolves the models directory:
    /// 1. Bundled .app: Contents/Resources/models/
    /// 2. Development: ./models/ relative to executable
    /// 3. Fallback: ~/.codescribe/models/
    pub fn new() -> Result<Self> {
        let models_dir = Self::resolve_models_dir()?;
        Ok(Self { models_dir })
    }

    fn resolve_models_dir() -> Result<PathBuf> {
        // Environment override
        if let Ok(path) = std::env::var("CODESCRIBE_MODELS_DIR") {
            let p = PathBuf::from(&path);
            if p.exists() {
                return Ok(p);
            }
        }

        let exe = std::env::current_exe().context("Failed to get executable path")?;
        let exe_dir = exe.parent().context("Failed to get executable directory")?;

        // 1. Bundled .app: Contents/MacOS/binary -> Contents/Resources/models/
        let bundled_path = exe_dir.join("../Resources/models");
        if bundled_path.exists() {
            return bundled_path
                .canonicalize()
                .context("Failed to canonicalize bundled models path");
        }

        // 2. Development: exe in target/debug/ -> ../../models/
        let dev_path = exe_dir.join("../../models");
        if dev_path.exists() {
            return dev_path
                .canonicalize()
                .context("Failed to canonicalize dev models path");
        }

        // 3. Direct ./models/ (running from repo root)
        let local_path = PathBuf::from("models");
        if local_path.exists() {
            return local_path
                .canonicalize()
                .context("Failed to canonicalize local models path");
        }

        // 4. Fallback: ~/.codescribe/models/ (lowercase!)
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let user_models = PathBuf::from(&home).join(".codescribe/models");
        fs::create_dir_all(&user_models).context("Failed to create user models directory")?;
        Ok(user_models)
    }

    pub fn get_model_path(&self, model_name: &str) -> PathBuf {
        // Check if it's an absolute path that exists
        let candidate = PathBuf::from(model_name);
        if candidate.is_absolute() && candidate.exists() {
            return candidate;
        }

        self.models_dir.join(model_name)
    }

    pub fn check_model_exists(&self, model_name: &str) -> bool {
        let path = self.get_model_path(model_name);
        path.join("tokenizer.json").exists()
    }

    pub fn list_models(&self) -> Result<Vec<String>> {
        if !self.models_dir.exists() {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        let entries =
            fs::read_dir(&self.models_dir).context("Failed to read models directory")?;
        for entry in entries {
            let entry = entry.context("Failed to read models directory entry")?;
            let path = entry.path();
            if path.is_dir() {
                // Check if it's a valid model (has tokenizer.json)
                if path.join("tokenizer.json").exists() {
                    if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                        out.push(name.to_string());
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn models_dir(&self) -> &PathBuf {
        &self.models_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_manager_list_models() {
        let manager = ModelManager::new().unwrap();
        let models = manager.list_models();
        assert!(models.is_ok());
        println!("Models dir: {}", manager.models_dir().display());
        println!("Found models: {:?}", models.unwrap());
    }

    #[test]
    fn test_model_manager_check_exists() {
        let manager = ModelManager::new().unwrap();
        // Non-existent model should return false
        assert!(!manager.check_model_exists("nonexistent-model-xyz"));
    }
}
