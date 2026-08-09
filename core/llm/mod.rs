//! LLM access layer: provider identity and wire families, per-lane credential
//! truth, subscription-account OAuth, model discovery, streaming transport,
//! and the AI formatting pass built on top of them.

pub mod account_auth;
pub mod ai_formatting;
pub mod client;
pub mod key_liveness;
pub mod lane_truth;
pub mod model_discovery;
pub mod provider;
pub mod responses_streaming_manager;
