//! LLM access layer: provider identity and wire families, per-lane credential
//! truth, subscription-account OAuth, model discovery, streaming transport,
//! and the AI formatting pass built on top of them.

/// Subscription-account OAuth foundation (Sign in with ChatGPT / Claude).
pub mod account_auth;
/// AI-powered punctuation/formatting pass over finished transcripts.
pub mod ai_formatting;
/// HTTP client for cloud STT / LLM multipart upload paths.
pub mod client;
/// Minimal API-key liveness probes for Settings (one cheap call per key).
pub mod key_liveness;
/// Canonical resolution of lane secrets, endpoints, and model ids.
pub mod lane_truth;
/// Live `/models` discovery for Settings pickers with last-good cache.
pub mod model_discovery;
/// Provider identity, wire families, and per-model capability policy.
pub mod provider;
/// SSE client for OpenAI-compatible `/v1/responses` streaming.
pub mod responses_streaming_manager;
