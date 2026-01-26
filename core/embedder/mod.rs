//! Text Embedder module - semantic embeddings using E5 (offline).
//!
//! Provides semantic text embeddings for RAG, similarity search, and context matching.
//! Uses a local/embedded multilingual-e5-large model (no runtime downloads).
//! Override with `CODESCRIBE_EMBEDDER_REPO=intfloat/multilingual-e5-small` (HF cache).
//!
//! ## Quick Start
//!
//! ```ignore
//! use codescribe_core::embedder;
//!
//! // Initialize embedder (embedded or local model)
//! embedder::init()?;
//!
//! // Embed text
//! let vec = embedder::embed("Hello world")?;  // Vec<f32>, 1024 dims
//!
//! // Batch embedding
//! let vecs = embedder::embed_batch(&["query 1", "query 2"])?;
//!
//! // Similarity
//! let sim = embedder::similarity(&vec_a, &vec_b);  // f32 cosine similarity
//! ```
//!
//! Created by M&K (c)2026 VetCoders

pub mod embedded;
pub mod engine;
pub mod singleton;

pub use engine::{EmbedderConfig, EmbedderEngine};
pub use singleton::{embed, embed_batch, init, is_initialized, similarity};

/// Default embedding dimension for E5 models
pub const EMBEDDING_DIM: usize = 1024;

/// Default model directory name (for local/embedded builds)
pub const DEFAULT_MODEL: &str = "e5-large";
