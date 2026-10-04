//! Anthropic Messages API wire specification, read from the official docs on
//! 2026-09-07. Every constant cites the page it was taken from; when the docs
//! and this file disagree, the docs win and this file is the bug.
//!
//! Self-contained by contract (`00_ATLAS.md §B.0`): only `serde_json`, no
//! imports from `provider.rs`, no HTTP.

/// Canonical registry spelling written into `settings.json` and env.
pub const CANONICAL: &str = "anthropic-messages";
/// Accepted alternative spellings, already lowercased.
pub const ALIASES: &[&str] = &["anthropic", "anthropic_messages"];
/// Picker label.
pub const DISPLAY_NAME: &str = "Anthropic (Messages)";
/// Root of the pages every constant below cites.
pub const DOCS_URL: &str = "https://platform.claude.com/docs/en/api/messages";
/// `POST /v1/messages` — pinned, never overridable from env or settings.
/// https://platform.claude.com/docs/en/api/messages ("POST https://api.anthropic.com/v1/messages")
pub const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
/// `GET /v1/models` — paginated with `after_id` / `has_more` / `last_id`.
/// https://platform.claude.com/docs/en/api/models-list
pub const MODELS_ENDPOINT: &str = "https://api.anthropic.com/v1/models";
/// Keychain account holding the API key.
pub const API_KEY_ACCOUNT: &str = "LLM_ANTHROPIC_API_KEY";
/// Authentication header. Anthropic does not accept `Authorization: Bearer`
/// for API keys.
/// https://platform.claude.com/docs/en/api/messages (Headers: `x-api-key`)
pub const AUTH_HEADER: &str = "x-api-key";
/// The raw key goes into [`AUTH_HEADER`] with no prefix.
pub const AUTH_VALUE_PREFIX: &str = "";
/// Mandatory on every request; current version per the versioning page.
/// https://platform.claude.com/docs/en/api/versioning (`anthropic-version: 2023-06-01`)
pub const EXTRA_HEADERS: &[(&str, &str)] = &[("anthropic-version", "2023-06-01")];
/// Formatting lane seed: cheapest current-generation model on 2026-09-07.
/// https://platform.claude.com/docs/en/about-claude/models/overview (Claude Sonnet 5, `claude-sonnet-5`)
pub const DEFAULT_FORMATTING_MODEL: &str = "claude-sonnet-5";
/// Assistive lane seed: current-generation Opus on 2026-09-07.
/// https://platform.claude.com/docs/en/about-claude/models/overview (Claude Opus 5, `claude-opus-5`)
pub const DEFAULT_ASSISTIVE_MODEL: &str = "claude-opus-5";

/// Extract `(id, display_name)` pairs from one `GET /v1/models` page.
///
/// Response shape per https://platform.claude.com/docs/en/api/models-list:
/// `{ "data": [{ "id", "display_name", "created_at", "type": "model", ... }],
///    "first_id", "has_more", "last_id" }`. Rows without an `id` are skipped
/// rather than turned into empty picker entries.
pub fn models_from_response(body: &serde_json::Value) -> Vec<(String, Option<String>)> {
    body.get("data")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let id = row.get("id")?.as_str()?.trim();
                    if id.is_empty() {
                        return None;
                    }
                    let display_name = row
                        .get("display_name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    Some((id.to_string(), display_name))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Smallest request `POST /v1/messages` accepts: `model`, `max_tokens`, and
/// one user message are the required fields per
/// https://platform.claude.com/docs/en/api/messages. One output token keeps a
/// liveness probe cheap without changing its auth semantics.
pub fn liveness_probe_body(model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "ping"}],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every constant, literally, so a drift from the docs is a one-line diff.
    #[test]
    fn constants_match_the_docs_read_on_2026_09_07() {
        assert_eq!(CANONICAL, "anthropic-messages");
        assert_eq!(ALIASES, &["anthropic", "anthropic_messages"]);
        assert_eq!(DISPLAY_NAME, "Anthropic (Messages)");
        assert_eq!(DOCS_URL, "https://platform.claude.com/docs/en/api/messages");
        assert_eq!(ENDPOINT, "https://api.anthropic.com/v1/messages");
        assert_eq!(MODELS_ENDPOINT, "https://api.anthropic.com/v1/models");
        assert_eq!(API_KEY_ACCOUNT, "LLM_ANTHROPIC_API_KEY");
        assert_eq!(AUTH_HEADER, "x-api-key");
        assert_eq!(AUTH_VALUE_PREFIX, "");
        assert_eq!(EXTRA_HEADERS, &[("anthropic-version", "2023-06-01")]);
        assert_eq!(DEFAULT_FORMATTING_MODEL, "claude-sonnet-5");
        assert_eq!(DEFAULT_ASSISTIVE_MODEL, "claude-opus-5");
    }

    /// Fixture is the example response from the models-list page.
    #[test]
    fn models_list_fixture_from_docs_parses_to_id_and_display_name() {
        let body = serde_json::json!({
            "data": [{
                "created_at": "2026-07-24T00:00:00Z",
                "display_name": "Claude Opus 5",
                "id": "claude-opus-5",
                "type": "model"
            }, {
                "id": "   ",
                "type": "model"
            }, {
                "id": "claude-haiku-4-5-20251001",
                "type": "model"
            }],
            "first_id": "claude-opus-5",
            "has_more": false,
            "last_id": "claude-haiku-4-5-20251001"
        });
        assert_eq!(
            models_from_response(&body),
            vec![
                (
                    "claude-opus-5".to_string(),
                    Some("Claude Opus 5".to_string())
                ),
                ("claude-haiku-4-5-20251001".to_string(), None),
            ]
        );
        assert!(models_from_response(&serde_json::json!({"error": {}})).is_empty());
    }

    /// The probe body carries exactly the three required fields.
    #[test]
    fn liveness_probe_body_is_the_minimal_required_request() {
        assert_eq!(
            liveness_probe_body("claude-sonnet-5"),
            serde_json::json!({
                "model": "claude-sonnet-5",
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "ping"}],
            })
        );
    }
}
