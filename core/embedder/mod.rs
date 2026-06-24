//! Text Embedder module - semantic embeddings using MiniLM (offline).
//!
//! Provides semantic text embeddings for RAG, similarity search, and context matching.
//! Uses a local/embedded paraphrase-multilingual-MiniLM-L12-v2 model (no runtime downloads by default).
//! Override with `CODESCRIBE_EMBEDDER_REPO=sentence-transformers/...` (HF cache).
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
//! let vec = embedder::embed("Hello world")?;  // Vec<f32>, 384 dims
//!
//! // Batch embedding
//! let vecs = embedder::embed_batch(&["query 1", "query 2"])?;
//!
//! // Similarity
//! let sim = embedder::similarity(&vec_a, &vec_b);  // f32 cosine similarity
//! ```

pub mod embedded;
pub mod engine;
pub mod singleton;

pub use engine::{EmbedderConfig, EmbedderEngine};
pub use singleton::{embed, embed_batch, init, is_initialized, similarity};

/// Default embedding dimension for MiniLM-L12-v2
pub const EMBEDDING_DIM: usize = 384;

/// Default model directory name (for local/embedded builds)
pub const DEFAULT_MODEL: &str = "minilm-l12-v2";
