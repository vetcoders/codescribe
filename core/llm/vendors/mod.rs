//! Vendor wire specifications: one self-contained module per pinned vendor.
//!
//! Each module is pure data plus two `serde_json` helpers and carries the docs
//! URL every constant was read from. Nothing here imports `provider.rs`; the
//! registry reads these rows, never the other way round.

/// Anthropic Messages API (`https://api.anthropic.com/v1/messages`).
pub mod anthropic;
/// Libraxis gateway on the OpenAI Responses protocol (`https://api.libraxis.com/v1/responses`).
pub mod libraxis;
/// OpenAI Responses API (`https://api.openai.com/v1/responses`).
pub mod openai;
/// xAI Responses API (`https://api.x.ai/v1/responses`).
pub mod xai;
