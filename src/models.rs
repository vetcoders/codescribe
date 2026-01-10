use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

use crate::config::Config;

pub struct ModelManager {
    models_dir: PathBuf,
}

impl ModelManager {
    pub fn new() -> Result<Self> {
        let env_dir = std::env::var("CODESCRIBE_MODELS_DIR").ok().map(PathBuf::from);
        let repo_models = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");

        let models_dir = env_dir
            .or_else(|| repo_models.exists().then(|| repo_models.clone()))
            .unwrap_or_else(|| Config::config_dir().join("models"));

        fs::create_dir_all(&models_dir).context("Failed to create models directory")?;

        Ok(Self { models_dir })
    }

    pub fn get_model_path(&self, model_name: &str) -> PathBuf {
        let candidate = PathBuf::from(model_name);
        if candidate.exists() {
            return candidate;
        }

        self.models_dir.join(model_name)
    }

    pub fn check_model_exists(&self, model_name: &str) -> bool {
        self.get_model_path(model_name).exists()
    }

}
