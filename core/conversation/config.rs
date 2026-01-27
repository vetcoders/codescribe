//! Configuration for Moshi conversation engine.
//!
//! Created by M&K (c)2026 VetCoders

use std::path::PathBuf;

use crate::hf_cache;

/// HuggingFace repos for Moshi models
const MOSHIKO_REPO: &str = "kyutai/moshiko-candle-q8";
const MOSHIKA_REPO: &str = "kyutai/moshika-candle-q8";

/// Required files for Moshi
const MOSHI_MODEL_FILE: &str = "model.q8.gguf";
const MOSHI_MIMI_FILE: &str = "tokenizer-e351c8d8-checkpoint125.safetensors";
const MOSHI_TOKENIZER_FILE: &str = "tokenizer_spm_32k_3.model";

/// Configuration for the Moshi conversation engine
#[derive(Debug, Clone)]
pub struct MoshiConfig {
    /// Path to Moshi model weights (e.g., moshiko-q8 or moshika-q8)
    pub model_path: PathBuf,

    /// Path to Mimi codec weights
    pub mimi_path: PathBuf,

    /// Path to tokenizer
    pub tokenizer_path: PathBuf,

    /// Temperature for sampling (0.0 = greedy, higher = more random)
    pub temperature: f32,

    /// Top-p (nucleus) sampling threshold
    pub top_p: f32,

    /// Maximum response length in frames
    pub max_response_frames: usize,

    /// Whether to use streaming mode
    pub streaming: bool,

    /// Voice selection: "moshiko" (male) or "moshika" (female)
    pub voice: String,

    /// Acoustic delay (frames) to prevent model hearing itself
    pub acoustic_delay: usize,
}

impl Default for MoshiConfig {
    fn default() -> Self {
        // Default to moshiko (male voice), paths can be overridden
        // All models in ~/.codescribe/models/ (unified path)
        let models_dir = directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".codescribe")
            .join("models");

        Self {
            // Moshiko LM weights (.gguf quantized)
            model_path: models_dir.join("moshiko-q8").join("model.q8.gguf"),
            // Mimi codec for moshi crate (NOT the same as candle-transformers mimi!)
            // This is "tokenizer-e351c8d8-checkpoint125.safetensors" from moshiko repo
            mimi_path: models_dir
                .join("moshiko-q8")
                .join("tokenizer-e351c8d8-checkpoint125.safetensors"),
            // SentencePiece tokenizer
            tokenizer_path: models_dir
                .join("moshiko-q8")
                .join("tokenizer_spm_32k_3.model"),
            temperature: 0.8,
            top_p: 0.9,
            max_response_frames: 500, // ~40 seconds at 80ms/frame
            streaming: true,
            voice: "moshiko".to_string(),
            acoustic_delay: 2,
        }
    }
}

impl MoshiConfig {
    /// Create config for Moshiko (male voice)
    pub fn moshiko() -> Self {
        Self::default()
    }

    /// Create config from HuggingFace cache for Moshiko (male voice)
    ///
    /// Looks for models in ~/.cache/huggingface/hub/
    pub fn moshiko_from_hf_cache() -> Option<Self> {
        let snapshot = hf_cache::find_snapshot(MOSHIKO_REPO, &[MOSHI_MODEL_FILE, MOSHI_MIMI_FILE])?;

        Some(Self {
            model_path: snapshot.join(MOSHI_MODEL_FILE),
            mimi_path: snapshot.join(MOSHI_MIMI_FILE),
            tokenizer_path: snapshot.join(MOSHI_TOKENIZER_FILE),
            voice: "moshiko".to_string(),
            ..Self::default()
        })
    }

    /// Create config from HuggingFace cache for Moshika (female voice)
    ///
    /// Looks for models in ~/.cache/huggingface/hub/
    pub fn moshika_from_hf_cache() -> Option<Self> {
        let snapshot = hf_cache::find_snapshot(MOSHIKA_REPO, &[MOSHI_MODEL_FILE, MOSHI_MIMI_FILE])?;

        Some(Self {
            model_path: snapshot.join(MOSHI_MODEL_FILE),
            mimi_path: snapshot.join(MOSHI_MIMI_FILE),
            tokenizer_path: snapshot.join(MOSHI_TOKENIZER_FILE),
            voice: "moshika".to_string(),
            ..Self::default()
        })
    }

    /// Create config for Moshika (female voice)
    pub fn moshika() -> Self {
        // All models in ~/.codescribe/models/ (unified path)
        let models_dir = directories::BaseDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".codescribe")
            .join("models");

        Self {
            model_path: models_dir.join("moshika-q8").join("model.q8.gguf"),
            // Mimi codec (same weights shared between moshiko/moshika)
            mimi_path: models_dir
                .join("moshika-q8")
                .join("tokenizer-e351c8d8-checkpoint125.safetensors"),
            tokenizer_path: models_dir
                .join("moshika-q8")
                .join("tokenizer_spm_32k_3.model"),
            voice: "moshika".to_string(),
            ..Self::default()
        }
    }

    /// Create config with custom model path
    pub fn with_model_path(mut self, path: PathBuf) -> Self {
        self.model_path = path.clone();
        self.tokenizer_path = path.join("tokenizer.json");
        self
    }

    /// Set temperature
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = temp.clamp(0.0, 2.0);
        self
    }

    /// Enable/disable streaming
    pub fn with_streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    /// Check if model files exist
    pub fn validate(&self) -> Result<(), String> {
        if !self.model_path.exists() {
            return Err(format!(
                "Moshi model not found at: {}. Run scripts/download-moshi.sh",
                self.model_path.display()
            ));
        }

        if !self.mimi_path.exists() {
            return Err(format!(
                "Mimi codec not found at: {}. Run scripts/download-moshi.sh",
                self.mimi_path.display()
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MoshiConfig::default();
        assert_eq!(config.voice, "moshiko");
        assert!(config.temperature > 0.0);
    }

    #[test]
    fn test_moshika_config() {
        let config = MoshiConfig::moshika();
        assert_eq!(config.voice, "moshika");
    }
}
