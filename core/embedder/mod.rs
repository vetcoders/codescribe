//! Text Embedder module - semantic embeddings using E5 via fastembed.
//!
//! Provides semantic text embeddings for RAG, similarity search, and context matching.
//! Uses fastembed with the multilingual-e5-large model for high-quality embeddings.
//!
//! ## Quick Start
//!
//! ```ignore
//! use codescribe_core::embedder;
//!
//! // Initialize embedder (downloads model on first use)
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

pub mod engine;
pub mod singleton;

pub use engine::{EmbedderConfig, EmbedderEngine};
pub use singleton::{embed, embed_batch, init, is_initialized, similarity};

/// Default embedding dimension for E5 models
pub const EMBEDDING_DIM: usize = 1024;

/// Model identifier for HuggingFace
pub const DEFAULT_MODEL: &str = "intfloat/multilingual-e5-large";
