//! OpenAI wire facts; registry policy identifiers are Codescribe-owned.
use serde_json::{Value, json};

// docs: https://developers.openai.com/api/reference/resources/responses/methods/create
// Codescribe identity/aliases/label retained from provider.rs; not vendor identifiers.
pub const CANONICAL: &str = "openai-responses";
// docs: https://developers.openai.com/api/reference/resources/responses/methods/create
pub const ALIASES: &[&str] = &["openai", "openai_responses"];
// docs: https://developers.openai.com/api/reference/resources/responses/methods/create
pub const DISPLAY_NAME: &str = "OpenAI (Responses)";
// docs: https://developers.openai.com/api/reference/resources/responses/methods/create
pub const DOCS_URL: &str =
    "https://developers.openai.com/api/reference/resources/responses/methods/create";
// docs: https://developers.openai.com/api/reference/resources/responses/methods/create
pub const ENDPOINT: &str = "https://api.openai.com/v1/responses";
// docs: https://developers.openai.com/api/reference/resources/models/methods/list
pub const MODELS_ENDPOINT: &str = "https://api.openai.com/v1/models";
// docs: https://developers.openai.com/api/reference/overview#authentication
// Codescribe Keychain account mandated by atlas D3, not OpenAI's env variable name.
pub const API_KEY_ACCOUNT: &str = "LLM_OPENAI_API_KEY";
// docs: https://developers.openai.com/api/reference/overview#authentication
pub const AUTH_HEADER: &str = "authorization";
// docs: https://developers.openai.com/api/reference/overview#authentication
pub const AUTH_VALUE_PREFIX: &str = "Bearer ";
// docs: https://developers.openai.com/api/reference/overview#authentication
// No mandatory vendor extras; transport supplies JSON Content-Type.
pub const EXTRA_HEADERS: &[(&str, &str)] = &[];
// docs: https://developers.openai.com/api/docs/models/gpt-4.1 (read 2026-09-07)
pub const DEFAULT_FORMATTING_MODEL: &str = "gpt-4.1";
// docs: https://developers.openai.com/api/docs/models/gpt-5.5 (read 2026-09-07)
pub const DEFAULT_ASSISTIVE_MODEL: &str = "gpt-5.5";

// docs: https://developers.openai.com/api/reference/resources/models/methods/list
// OpenAI supplies an id, not a display_name. Keep order and opaque ids unchanged.
// Policy: missing/non-array data yields no rows; malformed ids are skipped.
pub fn models_from_response(body: &Value) -> Vec<(String, Option<String>)> {
    body.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| model.get("id")?.as_str().map(|id| (id.to_owned(), None)))
        .collect()
}

// docs: https://developers.openai.com/api/reference/resources/responses/methods/create
// One-token cap is atlas B.0 policy, not a guarantee of model-specific acceptance
// or visible output: reasoning also consumes this budget. I1 must probe the API.
pub fn liveness_probe_body(model: &str) -> Value {
    json!({"model": model, "input": "ping", "max_output_tokens": 1})
}

// Speech wire pins; vendor REST audio documentation, verified 2026-09-08.
/// Vendor speech endpoint or Codescribe speech default.
pub const TTS_ENDPOINT: &str = "https://api.openai.com/v1/audio/speech";
/// Vendor speech endpoint or Codescribe speech default.
pub const STT_ENDPOINT: &str = "https://api.openai.com/v1/audio/transcriptions";
/// Vendor speech endpoint or Codescribe speech default.
pub const DEFAULT_TTS_MODEL: &str = "gpt-4o-mini-tts-2025-12-15";
/// Vendor speech endpoint or Codescribe speech default.
pub const DEFAULT_TTS_VOICE: &str = "cedar";
/// Vendor speech endpoint or Codescribe speech default.
pub const DEFAULT_STT_MODEL: &str = "gpt-4o-mini-transcribe";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_is_official_host() {
        assert_eq!(ENDPOINT, "https://api.openai.com/v1/responses");
        assert_eq!(MODELS_ENDPOINT, "https://api.openai.com/v1/models");
        assert_eq!(API_KEY_ACCOUNT, "LLM_OPENAI_API_KEY");
        assert_eq!(CANONICAL, "openai-responses");
        assert_eq!(ALIASES, &["openai", "openai_responses"]);
        assert_eq!(DISPLAY_NAME, "OpenAI (Responses)");
        assert_eq!(
            DOCS_URL,
            "https://developers.openai.com/api/reference/resources/responses/methods/create"
        );
        assert_eq!(AUTH_HEADER, "authorization");
        assert_eq!(AUTH_VALUE_PREFIX, "Bearer ");
        assert_eq!(EXTRA_HEADERS, &[] as &[(&str, &str)]);
        assert_eq!(DEFAULT_FORMATTING_MODEL, "gpt-4.1");
        assert_eq!(DEFAULT_ASSISTIVE_MODEL, "gpt-5.5");
    }

    #[test]
    fn models_fixture_from_docs_parses() {
        // docs: https://developers.openai.com/api/reference/resources/models/methods/list
        // MCP get_openapi_spec /models GET x-oaiMeta.examples.response, 2026-09-07.
        // Verbatim data; only invalid trailing comma after final object removed.
        let fixture: Value = serde_json::from_str(
            r#"{
  "object": "list",
  "data": [
    {
      "id": "model-id-0",
      "object": "model",
      "created": 1686935002,
      "owned_by": "organization-owner",
      "shutdown_date": null
    },
    {
      "id": "model-id-1",
      "object": "model",
      "created": 1686935002,
      "owned_by": "organization-owner",
      "shutdown_date": null
    },
    {
      "id": "model-id-2",
      "object": "model",
      "created": 1686935002,
      "owned_by": "openai",
      "shutdown_date": "2026-10-23"
    }
  ]
}"#,
        )
        .unwrap();
        assert_eq!(
            models_from_response(&fixture),
            vec![
                ("model-id-0".into(), None),
                ("model-id-1".into(), None),
                ("model-id-2".into(), None),
            ]
        );
    }

    #[test]
    fn malformed_models_do_not_invent_names_or_filter_ids() {
        for body in [Value::Null, json!({}), json!({"data": {}})] {
            assert!(models_from_response(&body).is_empty());
        }
        let body = json!({"data": [null, {}, {"id": 7},
            {"id": "future/id", "display_name": "not an OpenAI field"}]});
        assert_eq!(
            models_from_response(&body),
            vec![("future/id".into(), None)]
        );
    }

    #[test]
    fn liveness_probe_body_matches_docs_shape() {
        assert_eq!(
            liveness_probe_body("future/id"),
            json!({
                "model": "future/id", "input": "ping", "max_output_tokens": 1
            })
        );
    }
}
