//! xAI vendor pin. `serde_json` only; I1 wires this into the registry.

/// Codescribe id (not an xAI slug). docs: https://docs.x.ai/developers/rest-api-reference
pub const CANONICAL: &str = "xai-responses";
/// Existing registry spellings. docs do not list these.
pub const ALIASES: &[&str] = &["xai", "grok", "xai_responses"];
/// docs: https://docs.x.ai/overview
pub const DISPLAY_NAME: &str = "xAI (Grok)";
/// docs: https://docs.x.ai/developers/rest-api-reference
pub const DOCS_URL: &str = "https://docs.x.ai/developers/rest-api-reference";
/// Pinned Responses wire (D1). docs: https://docs.x.ai/developers/rest-api-reference/inference/responses
pub const ENDPOINT: &str = "https://api.x.ai/v1/responses";
/// No `display_name` on this shape. docs: https://docs.x.ai/developers/rest-api-reference/inference/models
pub const MODELS_ENDPOINT: &str = "https://api.x.ai/v1/models";
/// Codescribe Keychain. Vendor env is `XAI_API_KEY`. docs: https://docs.x.ai/developers/rest-api-reference
pub const API_KEY_ACCOUNT: &str = "LLM_XAI_API_KEY";
/// docs: https://docs.x.ai/developers/rest-api-reference
pub const AUTH_HEADER: &str = "authorization";
/// docs: https://docs.x.ai/developers/rest-api-reference
pub const AUTH_VALUE_PREFIX: &str = "Bearer ";
/// Bearer only. docs: https://docs.x.ai/developers/rest-api-reference
pub const EXTRA_HEADERS: &[(&str, &str)] = &[];
/// Seed 2026-09-07. docs: https://docs.x.ai/developers/grok-4-6
pub const DEFAULT_FORMATTING_MODEL: &str = "grok-4.6";
/// Same seed (D2: this module does not veto the lane). docs: https://docs.x.ai/developers/grok-4-6
pub const DEFAULT_ASSISTIVE_MODEL: &str = "grok-4.6";

/// Parse `GET /v1/models` (`data`) or `GET /v1/language-models` (`models`).
/// docs: https://docs.x.ai/developers/rest-api-reference/inference/models
pub fn models_from_response(body: &serde_json::Value) -> Vec<(String, Option<String>)> {
    body.get("data")
        .or_else(|| body.get("models"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let id = row.get("id")?.as_str()?.trim();
            (!id.is_empty()).then(|| (id.to_string(), None))
        })
        .collect()
}

/// Minimal create-response body. `max_tokens` is Chat Completions (deprecated).
/// docs: https://docs.x.ai/developers/rest-api-reference/inference/responses
pub fn liveness_probe_body(model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "input": "ping",
        "max_output_tokens": 1,
    })
}

// Speech wire pins; vendor REST audio documentation, verified 2026-09-08.
/// Vendor speech endpoint or Codescribe speech default.
pub const TTS_ENDPOINT: &str = "https://api.x.ai/v1/tts";
/// Vendor speech endpoint or Codescribe speech default.
pub const STT_ENDPOINT: &str = "https://api.x.ai/v1/stt";
/// Speech default; empty model means the vendor does not accept a model field.
pub const DEFAULT_TTS_MODEL: &str = "";
/// Vendor speech endpoint or Codescribe speech default.
pub const DEFAULT_TTS_VOICE: &str = "eve";
/// Speech default; empty model means the vendor does not accept a model field.
pub const DEFAULT_STT_MODEL: &str = "";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_is_official_host() {
        assert_eq!(ENDPOINT, "https://api.x.ai/v1/responses");
        assert!(ENDPOINT.starts_with("https://api.x.ai/"));
        assert_eq!(API_KEY_ACCOUNT, "LLM_XAI_API_KEY");
        assert_eq!(MODELS_ENDPOINT, "https://api.x.ai/v1/models");
        assert_eq!(CANONICAL, "xai-responses");
        assert_eq!(ALIASES, &["xai", "grok", "xai_responses"]);
        assert_eq!(DISPLAY_NAME, "xAI (Grok)");
        assert_eq!(DOCS_URL, "https://docs.x.ai/developers/rest-api-reference");
        assert_eq!(AUTH_HEADER, "authorization");
        assert_eq!(AUTH_VALUE_PREFIX, "Bearer ");
        assert!(EXTRA_HEADERS.is_empty());
        assert_eq!(DEFAULT_FORMATTING_MODEL, "grok-4.6");
        assert_eq!(DEFAULT_ASSISTIVE_MODEL, "grok-4.6");
    }

    #[test]
    fn models_fixture_from_docs_parses() {
        // Copied from https://docs.x.ai/developers/rest-api-reference/inference/models.md
        // GET /v1/models example, first `data` row + envelope (read 2026-09-07).
        let body = serde_json::json!({
            "data": [{
                "id": "latest",
                "aliases": [],
                "context_length": 131072,
                "created": 1776556800,
                "object": "model",
                "owned_by": "xai",
                "prompt_text_token_price": 12500,
                "cached_prompt_text_token_price": 2000,
                "prompt_image_token_price": 12500,
                "completion_text_token_price": 25000
            }],
            "object": "list"
        });
        assert_eq!(models_from_response(&body), vec![("latest".into(), None)]);
    }

    #[test]
    fn liveness_probe_body_matches_docs_shape() {
        let body = liveness_probe_body("grok-4.6");
        assert_eq!(body["model"], "grok-4.6");
        assert_eq!(body["input"], "ping");
        assert_eq!(body["max_output_tokens"], 1);
        assert!(body.get("max_tokens").is_none());
    }
}
