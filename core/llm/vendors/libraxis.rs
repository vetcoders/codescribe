//! Libraxis gateway wire specification (OpenAI Responses protocol), pinned on
//! 2026-09-07 from the Founder's live probe and the gateway source. There is no
//! public docs page: [`DOCS_URL`] points at the gateway repository.
//!
//! Self-contained by contract (`00_ATLAS.md §B.0` / `§B.3`): only `serde_json`,
//! no imports from `provider.rs`, no HTTP.

/// Canonical registry spelling written into `settings.json` and env.
pub const CANONICAL: &str = "libraxis-responses";
/// Accepted alternative spellings, already lowercased.
pub const ALIASES: &[&str] = &["libraxis", "lbrx", "libraxis_responses"];
/// Picker label.
pub const DISPLAY_NAME: &str = "Libraxis";
/// Gateway source; no public REST docs page existed on 2026-09-07.
pub const DOCS_URL: &str = "https://github.com/LibraxisAI/lbrx-services (branch feat/vista-brain-revival) + live probe 2026-09-07";
/// `POST /v1/responses` — pinned, never overridable from env or settings.
/// Live probe 2026-09-07 (Founder): the gateway answers the Responses shape here.
pub const ENDPOINT: &str = "https://api.libraxis.com/v1/responses";
/// `GET /v1/models` — OpenAI-compatible list, verified live with a key on
/// 2026-09-07 (HTTP 200); 401 without one.
pub const MODELS_ENDPOINT: &str = "https://api.libraxis.com/v1/models";
/// Keychain account holding the API key.
pub const API_KEY_ACCOUNT: &str = "LLM_LIBRAXIS_API_KEY";
/// Bearer authentication, OpenAI style.
pub const AUTH_HEADER: &str = "authorization";
/// Header value is `Bearer <key>`.
pub const AUTH_VALUE_PREFIX: &str = "Bearer ";
/// No extra headers.
pub const EXTRA_HEADERS: &[(&str, &str)] = &[];
/// Both lanes seed the gateway's default profile; `programmer`, `soap`,
/// `chat`, `suggestions` are aliases of the same model behind other profiles.
pub const DEFAULT_FORMATTING_MODEL: &str = "buddy";
/// See [`DEFAULT_FORMATTING_MODEL`].
pub const DEFAULT_ASSISTIVE_MODEL: &str = "buddy";
/// Every host the vendor has answered on; legacy endpoints on any of them
/// migrate to this vendor row instead of a Custom row.
pub const HOSTS: &[&str] = &["api.libraxis.com", "api.libraxis.cloud"];

/// Extract `(id, display_name)` pairs from one `GET /v1/models` page.
///
/// Shape verified live on 2026-09-07 (fixture `fixtures/libraxis_models_live_2026-09-07.json`):
/// `{ "object": "list", "data": [{ "id", "object": "model", "owned_by",
/// "is_alias", "modalities", "capability_matrix", ... }] }`. Only `data[].id`
/// is read; rows without one are skipped.
pub fn models_from_response(body: &serde_json::Value) -> Vec<(String, Option<String>)> {
    body.get("data")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let id = row.get("id")?.as_str()?.trim();
                    (!id.is_empty()).then(|| (id.to_string(), None))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Smallest Responses request the gateway accepts: one `input_text` item and a
/// one-token output budget, no reasoning.
///
/// On 2026-09-07 the gateway answered `503 {"error":{"code":"no_eligible_provider"}}`
/// for a keyed probe: a routing state, not an auth failure. The liveness probe
/// must surface `error.code` in its message so Test connection says which.
pub fn liveness_probe_body(model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "input": [{"role": "user", "content": [{"type": "input_text", "text": "ping"}]}],
        "max_output_tokens": 1,
        "stream": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every constant, literally, so a drift from the gateway is a one-line diff.
    #[test]
    fn constants_match_the_gateway_facts_pinned_on_2026_09_07() {
        assert_eq!(CANONICAL, "libraxis-responses");
        assert_eq!(ALIASES, &["libraxis", "lbrx", "libraxis_responses"]);
        assert_eq!(DISPLAY_NAME, "Libraxis");
        assert!(DOCS_URL.starts_with("https://github.com/LibraxisAI/lbrx-services"));
        assert_eq!(ENDPOINT, "https://api.libraxis.com/v1/responses");
        assert_eq!(MODELS_ENDPOINT, "https://api.libraxis.com/v1/models");
        assert_eq!(API_KEY_ACCOUNT, "LLM_LIBRAXIS_API_KEY");
        assert_eq!(AUTH_HEADER, "authorization");
        assert_eq!(AUTH_VALUE_PREFIX, "Bearer ");
        assert!(EXTRA_HEADERS.is_empty());
        assert_eq!(DEFAULT_FORMATTING_MODEL, "buddy");
        assert_eq!(DEFAULT_ASSISTIVE_MODEL, "buddy");
        assert_eq!(HOSTS, &["api.libraxis.com", "api.libraxis.cloud"]);
    }

    /// Fixture is an OpenAI-compatible list; unverified live (401 without key).
    #[test]
    fn openai_compatible_models_fixture_parses_to_ids() {
        let body = serde_json::json!({
            "object": "list",
            "data": [
                {"id": "buddy", "object": "model"},
                {"id": "programmer", "object": "model"},
                {"object": "model"}
            ]
        });
        assert_eq!(
            models_from_response(&body),
            vec![
                ("buddy".to_string(), None),
                ("programmer".to_string(), None)
            ]
        );
    }

    /// The probe body is the one-token Responses ping.
    #[test]
    fn liveness_probe_body_is_a_one_token_responses_ping() {
        let body = liveness_probe_body("buddy");
        assert_eq!(body["model"], "buddy");
        assert_eq!(body["max_output_tokens"], 1);
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    }
}
