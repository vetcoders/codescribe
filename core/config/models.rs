//! Runtime fallback model management for Whisper models.
//!
//! This module owns the runtime Whisper fallback truth for the `develop`
//! branch. If embedded Whisper is unavailable, every caller should resolve a
//! model from here instead of re-implementing its own precedence rules.
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};

use crate::hf_cache;

/// Default Whisper model name used for runtime fallback lookup.
pub const DEFAULT_MODEL: &str = "whisper-large-v3-turbo-mlx-q8";
pub const DEFAULT_WHISPER_REPO: &str = "LibraxisAI/whisper-large-v3-turbo-mlx-q8";

const REQUIRED_MODEL_FILES: [&str; 3] = ["config.json", "tokenizer.json", "mel_filters.npz"];
const REQUIRED_MODEL_WEIGHTS: [&str; 2] = ["weights.safetensors", "model.safetensors"];

fn canonicalize_or_self(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn is_complete_whisper_model_dir(path: &Path) -> bool {
    REQUIRED_MODEL_FILES
        .iter()
        .all(|name| path.join(name).exists())
        && REQUIRED_MODEL_WEIGHTS
            .iter()
            .any(|name| path.join(name).exists())
}

fn hf_snapshot_for_model(model_ref: &str) -> Option<PathBuf> {
    let trimmed = model_ref.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains('/') {
        return hf_cache::find_snapshot_with_any(
            trimmed,
            &REQUIRED_MODEL_FILES,
            &REQUIRED_MODEL_WEIGHTS,
        );
    }

    if trimmed == DEFAULT_MODEL {
        return hf_cache::find_snapshot_with_any(
            DEFAULT_WHISPER_REPO,
            &REQUIRED_MODEL_FILES,
            &REQUIRED_MODEL_WEIGHTS,
        );
    }

    None
}

pub struct ModelManager {
    models_dir: PathBuf,
}

impl ModelManager {
    /// Create a new ModelManager for runtime fallback lookup.
    ///
    /// Resolves the runtime models directory:
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
        let local_path = PathBuf::from("../../models");
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

    pub fn resolve_model_reference(&self, model_ref: &str) -> PathBuf {
        let candidate = PathBuf::from(model_ref);
        if candidate.exists() {
            return canonicalize_or_self(candidate);
        }

        self.models_dir.join(model_ref)
    }

    pub fn check_model_exists(&self, model_name: &str) -> bool {
        let path = self.resolve_model_reference(model_name);
        is_complete_whisper_model_dir(&path)
    }

    pub fn list_models(&self) -> Result<Vec<String>> {
        if !self.models_dir.exists() {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        let entries = fs::read_dir(&self.models_dir).context("Failed to read models directory")?;
        for entry in entries {
            let entry = entry.context("Failed to read models directory entry")?;
            let path = entry.path();
            if path.is_dir() {
                // Only advertise fully usable Whisper models, not half-downloaded shells.
                if is_complete_whisper_model_dir(&path)
                    && let Some(name) = path.file_name().and_then(|s| s.to_str())
                {
                    out.push(name.to_string());
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

/// Resolve the authoritative runtime Whisper fallback model path.
///
/// Precedence:
/// 1. Explicit `CODESCRIBE_MODEL_PATH`
/// 2. Configured local model path / models-dir alias
/// 3. Configured Hugging Face repo snapshot
/// 4. Default models-dir alias (`whisper-large-v3-turbo-mlx-q8`)
/// 5. Default Hugging Face snapshot (`LibraxisAI/whisper-large-v3-turbo-mlx-q8`)
pub fn resolve_runtime_whisper_model_path(configured_model: Option<&str>) -> Result<PathBuf> {
    if let Ok(path) = std::env::var("CODESCRIBE_MODEL_PATH") {
        let candidate = PathBuf::from(path.trim());
        if is_complete_whisper_model_dir(&candidate) {
            return Ok(canonicalize_or_self(candidate));
        }
    }

    let manager = ModelManager::new()?;
    let configured_model = configured_model
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(model_ref) = configured_model {
        let local_candidate = manager.resolve_model_reference(model_ref);
        if is_complete_whisper_model_dir(&local_candidate) {
            return Ok(canonicalize_or_self(local_candidate));
        }

        if let Some(snapshot) = hf_snapshot_for_model(model_ref) {
            return Ok(snapshot);
        }
    }

    let default_local = manager.get_model_path(DEFAULT_MODEL);
    if is_complete_whisper_model_dir(&default_local) {
        return Ok(canonicalize_or_self(default_local));
    }

    if let Some(snapshot) = hf_snapshot_for_model(DEFAULT_MODEL) {
        return Ok(snapshot);
    }

    Err(anyhow!(
        "Whisper runtime fallback model not available.\n\
         Embedded Whisper is preferred, but this build/runtime path still needs a complete local model.\n\
         Set CODESCRIBE_MODEL_PATH, configure LOCAL_MODEL, or warm the Hugging Face cache.\n\n\
         Download with: hf download {}",
        DEFAULT_WHISPER_REPO
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: these tests run under `serial` and restore the prior env.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: these tests run under `serial` and restore the prior env.
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.prev {
                // SAFETY: these tests run under `serial` and restore the prior env.
                unsafe { std::env::set_var(self.key, prev) };
            } else {
                // SAFETY: these tests run under `serial` and restore the prior env.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }

    fn create_complete_whisper_model(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("config.json"), "{}").unwrap();
        fs::write(path.join("tokenizer.json"), "{}").unwrap();
        fs::write(path.join("mel_filters.npz"), "npz").unwrap();
        fs::write(path.join("model.safetensors"), "weights").unwrap();
    }

    #[test]
    #[serial]
    fn test_model_manager_list_models() {
        let manager = ModelManager::new().unwrap();
        let models = manager.list_models();
        assert!(models.is_ok());
        println!("Models dir: {}", manager.models_dir().display());
        println!("Found models: {:?}", models.unwrap());
    }

    #[test]
    #[serial]
    fn test_model_manager_check_exists() {
        let manager = ModelManager::new().unwrap();
        // Non-existent model should return false
        assert!(!manager.check_model_exists("nonexistent-model-xyz"));
    }

    #[test]
    #[serial]
    fn test_model_manager_custom_models() {
        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().join("../../models");
        fs::create_dir_all(&models_dir).unwrap();

        let model_names = [
            "whisper-base-mlx-q8",
            "whisper-medium-mlx-q8",
            "whisper-large-v3-turbo-mlx-q8",
        ];

        for name in &model_names {
            let model_path = models_dir.join(name);
            create_complete_whisper_model(&model_path);
        }

        let _models_dir = EnvGuard::set("CODESCRIBE_MODELS_DIR", &models_dir);

        let manager = ModelManager::new().unwrap();
        let models = manager.list_models().unwrap();

        for name in &model_names {
            assert!(models.contains(&name.to_string()));
            assert!(manager.check_model_exists(name));
        }
    }

    #[test]
    #[serial]
    fn test_model_manager_rejects_incomplete_whisper_models() {
        let temp_dir = TempDir::new().unwrap();
        let models_dir = temp_dir.path().join("models");
        let complete = models_dir.join("complete-whisper");
        let incomplete = models_dir.join("incomplete-whisper");

        create_complete_whisper_model(&complete);
        fs::create_dir_all(&incomplete).unwrap();
        fs::write(incomplete.join("tokenizer.json"), "{}").unwrap();

        let _models_dir = EnvGuard::set("CODESCRIBE_MODELS_DIR", &models_dir);
        let manager = ModelManager::new().unwrap();

        assert!(manager.check_model_exists("complete-whisper"));
        assert!(!manager.check_model_exists("incomplete-whisper"));
        assert_eq!(manager.list_models().unwrap(), vec!["complete-whisper"]);
    }

    #[test]
    #[serial]
    fn resolve_runtime_whisper_model_path_prefers_complete_env_override() {
        let temp_dir = TempDir::new().unwrap();
        let env_model = temp_dir.path().join("env-model");
        let models_dir = temp_dir.path().join("models");
        let bundled_default = models_dir.join(DEFAULT_MODEL);

        create_complete_whisper_model(&env_model);
        create_complete_whisper_model(&bundled_default);

        let _env_override = EnvGuard::set("CODESCRIBE_MODEL_PATH", &env_model);
        let _models_dir = EnvGuard::set("CODESCRIBE_MODELS_DIR", &models_dir);
        let _hf_cache = EnvGuard::unset("CODESCRIBE_HF_CACHE");

        let resolved = resolve_runtime_whisper_model_path(Some(DEFAULT_MODEL)).unwrap();
        assert_eq!(resolved, canonicalize_or_self(env_model));
    }

    #[test]
    #[serial]
    fn resolve_runtime_whisper_model_path_uses_hf_repo_id_from_cache() {
        let temp_dir = TempDir::new().unwrap();
        let hf_cache = temp_dir.path().join("hf-cache");
        let snapshot = hf_cache
            .join("models--VetCoders--custom-whisper")
            .join("snapshots")
            .join("abc123");

        create_complete_whisper_model(&snapshot);

        let _models_dir = EnvGuard::set(
            "CODESCRIBE_MODELS_DIR",
            temp_dir.path().join("models").as_path(),
        );
        let _env_override = EnvGuard::unset("CODESCRIBE_MODEL_PATH");
        let _hf_cache = EnvGuard::set("CODESCRIBE_HF_CACHE", &hf_cache);

        let resolved =
            resolve_runtime_whisper_model_path(Some("VetCoders/custom-whisper")).unwrap();
        assert_eq!(resolved, snapshot);
    }
}
