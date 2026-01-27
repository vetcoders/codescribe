//! Embedder Engine - fastembed wrapper for text embeddings.
//!
//! Provides text embeddings using fastembed with E5 models.
//! Supports query and passage prefixes for optimal retrieval performance.
//!
//! Created by M&K (c)2026 VetCoders

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use tracing::info;

/// Configuration for the embedder
#[derive(Debug, Clone)]
pub struct EmbedderConfig {
    /// Model to use (default: multilingual-e5-large)
    pub model: EmbeddingModel,
    /// Whether to show download progress
    pub show_download_progress: bool,
    /// Cache directory for models
    pub cache_dir: Option<String>,
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        Self {
            model: EmbeddingModel::MultilingualE5Large,
            show_download_progress: true,
            cache_dir: None,
        }
    }
}

impl EmbedderConfig {
    /// Create config for a specific model
    pub fn with_model(model: EmbeddingModel) -> Self {
        Self {
            model,
            ..Default::default()
        }
    }

    /// Use smaller model for faster inference
    pub fn small() -> Self {
        Self {
            model: EmbeddingModel::MultilingualE5Small,
            ..Default::default()
        }
    }

    /// Use base model for balanced performance
    pub fn base() -> Self {
        Self {
            model: EmbeddingModel::MultilingualE5Base,
            ..Default::default()
        }
    }
}

/// Text embedding engine using fastembed
pub struct EmbedderEngine {
    model: TextEmbedding,
    config: EmbedderConfig,
}

impl EmbedderEngine {
    /// Create a new embedder with default config
    pub fn new() -> Result<Self> {
        Self::with_config(EmbedderConfig::default())
    }

    /// Create with custom configuration
    pub fn with_config(config: EmbedderConfig) -> Result<Self> {
        info!("Initializing embedder with model: {:?}", config.model);

        let mut init_options = InitOptions::new(config.model.clone());

        if config.show_download_progress {
            init_options = init_options.with_show_download_progress(true);
        }

        if let Some(ref cache_dir) = config.cache_dir {
            init_options = init_options.with_cache_dir(cache_dir.into());
        }

        let model = TextEmbedding::try_new(init_options)
            .context("Failed to initialize text embedding model")?;

        info!("Embedder initialized successfully");

        Ok(Self { model, config })
    }

    /// Embed a single text
    ///
    /// For queries (search), the text is automatically prefixed with "query: "
    /// For passages (documents), use `embed_passage` instead
    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>> {
        let query = format!("query: {}", text);
        let embeddings = self
            .model
            .embed(vec![query], None)
            .context("Failed to generate embedding")?;

        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embedding generated"))
    }

    /// Embed a passage (document) for indexing
    ///
    /// Passages are prefixed with "passage: " for optimal retrieval
    pub fn embed_passage(&mut self, text: &str) -> Result<Vec<f32>> {
        let passage = format!("passage: {}", text);
        let embeddings = self
            .model
            .embed(vec![passage], None)
            .context("Failed to generate passage embedding")?;

        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embedding generated"))
    }

    /// Embed multiple texts at once (queries)
    pub fn embed_batch(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let queries: Vec<String> = texts.iter().map(|t| format!("query: {}", t)).collect();
        let string_refs: Vec<&str> = queries.iter().map(|s| s.as_str()).collect();

        self.model
            .embed(string_refs, None)
            .context("Failed to generate batch embeddings")
    }

    /// Embed multiple passages at once (documents)
    pub fn embed_passages(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let passages: Vec<String> = texts.iter().map(|t| format!("passage: {}", t)).collect();
        let string_refs: Vec<&str> = passages.iter().map(|s| s.as_str()).collect();

        self.model
            .embed(string_refs, None)
            .context("Failed to generate passage embeddings")
    }

    /// Calculate cosine similarity between two embeddings
    pub fn similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }

        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }

        dot / (norm_a * norm_b)
    }

    /// Get embedding dimension
    pub fn dimension(&self) -> usize {
        // E5 models output 1024-dim for large, 768 for base, 384 for small
        match self.config.model {
            EmbeddingModel::MultilingualE5Large => 1024,
            EmbeddingModel::MultilingualE5Base => 768,
            EmbeddingModel::MultilingualE5Small => 384,
            _ => 1024, // Default assumption
        }
    }

    /// Get the model being used
    pub fn model(&self) -> &EmbeddingModel {
        &self.config.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = EmbedderEngine::similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = EmbedderEngine::similarity(&a, &b);
        assert!(sim.abs() < 0.001);
    }

    #[test]
    fn test_similarity_opposite() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        let sim = EmbedderEngine::similarity(&a, &b);
        assert!((sim + 1.0).abs() < 0.001);
    }

    #[test]
    fn test_similarity_empty() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        let sim = EmbedderEngine::similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }
}
