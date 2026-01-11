use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

use crate::config::Config;

/// Default bundled model name
pub const DEFAULT_MODEL: &str = "whisper-large-v3-turbo-mlx-q8";

pub struct ModelManager {
    models_dir: PathBuf,
    bundle_dir: Option<PathBuf>,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        let env_dir = std::env::var("CODESCRIBE_MODELS_DIR")
            .ok()
            .map(PathBuf::from);
        let repo_models = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");

        let models_dir = env_dir
            .or_else(|| repo_models.exists().then(|| repo_models.clone()))
            .unwrap_or_else(|| Config::config_dir().join("models"));

        fs::create_dir_all(&models_dir).context("Failed to create models directory")?;

        // Try to find bundle resources path (macOS app bundle)
        let bundle_dir = Self::find_bundle_resources_dir();

        Ok(Self { models_dir, bundle_dir })
    }

    /// Find the Resources directory in a macOS app bundle
    fn find_bundle_resources_dir() -> Option<PathBuf> {
        // Get current executable path
        let exe = std::env::current_exe().ok()?;

        // In macOS bundle: /path/to/App.app/Contents/MacOS/binary
        // Resources are at: /path/to/App.app/Contents/Resources/
        let contents = exe.parent()?.parent()?;
        let resources = contents.join("Resources");

        if resources.exists() {
            Some(resources)
        } else {
            None
        }
    }

    pub fn get_model_path(&self, model_name: &str) -> PathBuf {
        // 1. Check if it's an absolute path that exists
        let candidate = PathBuf::from(model_name);
        if candidate.exists() {
            return candidate;
        }

        // 2. Check bundle resources first (for bundled app)
        if let Some(ref bundle_dir) = self.bundle_dir {
            let bundle_path = bundle_dir.join("models").join(model_name);
            if bundle_path.exists() {
                return bundle_path;
            }
        }

        // 3. Fall back to user models directory
        self.models_dir.join(model_name)
    }

    pub fn check_model_exists(&self, model_name: &str) -> bool {
        self.get_model_path(model_name).exists()
    }

    pub fn list_models(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let entries = fs::read_dir(&self.models_dir).context("Failed to read models directory")?;
        for entry in entries {
            let entry = entry.context("Failed to read models directory entry")?;
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    out.push(name.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_manager_list_models() {
        // This test ensures list_models() is not dead code
        // (it's used by tauri-app but clippy doesn't see cross-workspace usage)
        let manager = ModelManager::new().unwrap();
        let models = manager.list_models();
        assert!(models.is_ok());
    }

    #[test]
    fn test_model_manager_check_exists() {
        let manager = ModelManager::new().unwrap();
        // Non-existent model should return false
        assert!(!manager.check_model_exists("nonexistent-model-xyz"));
    }
}
