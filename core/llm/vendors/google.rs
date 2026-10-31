//! Native Gemini GenerateContent wire data and request shaping.
//! Reference: https://ai.google.dev/api/generate-content and
//! https://ai.google.dev/gemini-api/docs/function-calling.

use serde_json::{Value, json};

/// Persisted vendor identity.
pub const CANONICAL: &str = "google-gemini";
/// Accepted vendor spellings. CCA hosts must use a Custom provider row.
pub const ALIASES: &[&str] = &["gemini", "google", "antigravity", "agy", "google_gemini"];
/// Provider picker label.
pub const DISPLAY_NAME: &str = "Google Gemini";
/// Official GenerateContent documentation.
pub const DOCS_URL: &str = "https://ai.google.dev/api/generate-content";
/// Standard API base; the model is a path parameter for each request.
pub const ENDPOINT: &str = "https://generativelanguage.googleapis.com/v1beta";
/// Codescribe Keychain account.
pub const API_KEY_ACCOUNT: &str = "LLM_GOOGLE_API_KEY";
/// Initial model seeds; discovery remains authoritative.
pub const DEFAULT_FORMATTING_MODEL: &str = "gemini-2.5-flash";
/// Initial Agent model seed.
pub const DEFAULT_ASSISTIVE_MODEL: &str = "gemini-2.5-pro";

/// Preserve explicit version/mount paths, including CCA's `v1internal`.
/// Switching from another wire removes only its terminal operation suffix.
pub fn normalize_endpoint(raw: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(raw.trim()) else {
        return raw.trim().to_string();
    };
    let mut path = url.path().trim_end_matches('/').to_string();
    for suffix in [
        "/v1/responses",
        "/v1/messages",
        "/v1/chat/completions",
        "/v1/completions",
    ] {
        if path.ends_with(suffix) {
            path.truncate(path.len() - suffix.len());
            break;
        }
    }
    if path.is_empty() {
        path.push_str("/v1beta");
    }
    url.set_path(&path);
    url.to_string()
}

/// Model-specific operation URL. The model is escaped as one path segment.
pub fn generate_content_endpoint(base: &str, model: &str, streaming: bool) -> String {
    let Ok(mut url) = reqwest::Url::parse(&normalize_endpoint(base)) else {
        return base.to_string();
    };
    let path = url.path().to_string();
    let operation = if streaming {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    if is_cloud_code_assist(base) {
        let prefix = path
            .split(':')
            .next()
            .unwrap_or(&path)
            .trim_end_matches('/');
        url.set_path(&format!("{prefix}:{operation}"));
    } else {
        let root = path
            .split("/models/")
            .next()
            .unwrap_or(&path)
            .trim_end_matches('/');
        url.set_path(root);
        if let Ok(mut segments) = url.path_segments_mut() {
            segments.push("models").push(&format!(
                "{}:{operation}",
                model.strip_prefix("models/").unwrap_or(model)
            ));
        }
    }
    let query: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !matches!(key.as_ref(), "alt" | "project" | "antigravity"))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    if !query.is_empty() || streaming {
        let mut pairs = url.query_pairs_mut();
        pairs.extend_pairs(query);
        if streaming {
            pairs.append_pair("alt", "sse");
        }
    }
    url.to_string()
}

/// CCA is explicitly selected by its internal operation path, never by OAuth
/// alone. This also permits a local mock/proxy without hardcoding its host.
pub fn is_cloud_code_assist(base: &str) -> bool {
    reqwest::Url::parse(base).is_ok_and(|url| url.path().contains("v1internal"))
}

/// Numeric loopback only: a fixture must not rely on a hostname resolving to
/// the same address later when credentialed HTTP is sent.
pub fn is_loopback_endpoint(base: &str) -> bool {
    reqwest::Url::parse(base).is_ok_and(|url| {
        url.host_str().is_some_and(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        })
    })
}

/// Wrap the native payload for CCA. The Custom endpoint's `project` query
/// parameter supplies the project; `antigravity=true` selects agy's envelope.
/// These non-secret routing values never become credentials or a second owner.
pub fn prepare_request(
    base: &str,
    model: &str,
    body: Value,
    streaming: bool,
) -> Result<(String, Value), String> {
    let url = reqwest::Url::parse(base).map_err(|_| "Invalid Google endpoint".to_string())?;
    let loopback = is_loopback_endpoint(base);
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("Google endpoint requires HTTPS (HTTP is permitted only for a numeric loopback fixture), without user info or a fragment".into());
    }
    // Keys live in headers; refusing URL credentials also keeps diagnostics safe.
    if url
        .query_pairs()
        .any(|(key, _)| matches!(key.as_ref(), "key" | "access_token" | "api_key"))
    {
        return Err(
            "Google credentials must use Keychain or environment, not endpoint query parameters"
                .into(),
        );
    }
    let endpoint = generate_content_endpoint(base, model, streaming);
    if !is_cloud_code_assist(base) {
        return Ok((endpoint, body));
    }
    let project = url
        .query_pairs()
        .find(|(key, _)| key == "project")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "Cloud Code Assist Custom endpoint requires ?project=<project-id>".to_string()
        })?;
    let antigravity = url
        .query_pairs()
        .any(|(key, value)| key == "antigravity" && value == "true");
    let mut envelope = json!({"project": project, "model": model, "request": body});
    if antigravity {
        envelope["userAgent"] = json!("antigravity");
        envelope["requestType"] = json!("agent");
        envelope["requestId"] = json!(uuid::Uuid::new_v4().to_string());
    }
    Ok((endpoint, envelope))
}

/// Models collection URL for a standard GenerateContent host.
pub fn models_endpoint(base: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(&normalize_endpoint(base)) else {
        return base.to_string();
    };
    let path = url
        .path()
        .split("/models/")
        .next()
        .unwrap_or(url.path())
        .trim_end_matches('/');
    let path = path.strip_suffix("/models").unwrap_or(path);
    url.set_path(&format!("{path}/models"));
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

/// Decode Google's model list and omit entries that cannot generate content.
pub fn models_from_response(body: &Value) -> Vec<(String, Option<String>)> {
    body.get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            if let Some(methods) = row
                .get("supportedGenerationMethods")
                .and_then(Value::as_array)
                && !methods.iter().any(|method| method == "generateContent")
            {
                return None;
            }
            let name = row
                .get("name")?
                .as_str()?
                .trim()
                .strip_prefix("models/")
                .unwrap_or(row["name"].as_str()?.trim());
            if name.is_empty() {
                return None;
            }
            Some((
                name.to_string(),
                row.get("displayName")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            ))
        })
        .collect()
}

/// Small native request used only by the explicit API-key Test action.
pub fn liveness_probe_body(_model: &str) -> Value {
    json!({"contents":[{"role":"user","parts":[{"text":"ping"}]}],"generationConfig":{"maxOutputTokens":1}})
}

/// Convert JSON Schema tool parameters into the Gemini OpenAPI Schema subset.
/// Properties are schema nodes; property *names* and enum/default data are not
/// traversed as keywords. Local references are expanded, cycles fail explicitly.
pub fn normalize_schema_for_gemini(schema: &Value) -> Result<Value, String> {
    normalize_schema_node(schema, schema, &mut Vec::new(), 0)
}

fn normalize_schema_node(
    node: &Value,
    root: &Value,
    refs: &mut Vec<String>,
    depth: usize,
) -> Result<Value, String> {
    if depth > 64 {
        return Err("Google tool schema exceeds nesting limit".into());
    }
    let Some(object) = node.as_object() else {
        return Err("Google tool schema must be an object".into());
    };
    let mut source = object.clone();
    if let Some(reference) = source.remove("$ref") {
        let reference = reference
            .as_str()
            .ok_or("Invalid Google tool schema reference")?;
        if !reference.starts_with("#/") || refs.iter().any(|seen| seen == reference) {
            return Err("Google tool schema has an external or recursive reference".into());
        }
        let target = root
            .pointer(&reference[1..])
            .ok_or("Unresolved Google tool schema reference")?;
        refs.push(reference.to_string());
        let expanded = normalize_schema_node(target, root, refs, depth + 1)?;
        refs.pop();
        let mut expanded = expanded
            .as_object()
            .cloned()
            .ok_or("Invalid referenced schema")?;
        expanded.extend(source);
        source = expanded;
    }
    for key in [
        "$schema",
        "$id",
        "$defs",
        "definitions",
        "additionalProperties",
        "default",
        "examples",
    ] {
        source.remove(key);
    }
    if let Some(kind) = source.remove("type") {
        if let Value::Array(types) = kind {
            let nullable = types.iter().any(|kind| kind == "null");
            let kinds: Vec<Value> = types.into_iter().filter(|kind| kind != "null").collect();
            if kinds.len() == 1 {
                source.insert("type".into(), kinds[0].clone());
            } else if !kinds.is_empty() {
                source.insert(
                    "anyOf".into(),
                    Value::Array(kinds.into_iter().map(|kind| json!({"type":kind})).collect()),
                );
            }
            if nullable {
                source.insert("nullable".into(), json!(true));
            }
        } else {
            source.insert("type".into(), kind);
        }
    }
    if let Some(Value::Object(properties)) = source.get_mut("properties") {
        for property in properties.values_mut() {
            *property = normalize_schema_node(property, root, refs, depth + 1)?;
        }
    }
    if let Some(items) = source.get_mut("items") {
        *items = normalize_schema_node(items, root, refs, depth + 1)?;
    }
    if let Some(one_of) = source.remove("oneOf") {
        source.insert("anyOf".into(), one_of);
    }
    if source.contains_key("allOf") {
        return Err("Google tool schema allOf needs explicit expansion".into());
    }
    if let Some(Value::Array(variants)) = source.get_mut("anyOf") {
        for variant in variants {
            *variant = normalize_schema_node(variant, root, refs, depth + 1)?;
        }
    }
    Ok(Value::Object(source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_schema_normalizes_nullable_refs_and_preserves_property_names() {
        let schema = json!({"$schema":"https://json-schema.org/draft/2020-12/schema", "$defs":{"arg":{"type":["string","null"],"description":"optional","enum":["a","b"]}},"type":"object","additionalProperties":false,"required":["type"],"properties":{"type":{"$ref":"#/$defs/arg"},"additionalProperties":{"type":"boolean"}}});
        let normalized = normalize_schema_for_gemini(&schema).unwrap();
        assert_eq!(normalized["required"], json!(["type"]));
        assert_eq!(
            normalized["properties"]["type"],
            json!({"type":"string","nullable":true,"description":"optional","enum":["a","b"]})
        );
        assert_eq!(
            normalized["properties"]["additionalProperties"]["type"],
            "boolean"
        );
        assert!(normalized.get("$schema").is_none());
        assert!(normalized.get("additionalProperties").is_none());
        assert!(
            normalize_schema_for_gemini(
                &json!({"$ref":"#/$defs/cycle","$defs":{"cycle":{"$ref":"#/$defs/cycle"}}})
            )
            .is_err()
        );
    }

    #[test]
    fn google_endpoints_and_cca_envelope_keep_routing_explicit() {
        assert_eq!(
            generate_content_endpoint(ENDPOINT, "gemini-2.5-pro", true),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
        let (url, body) = prepare_request(
            "https://cloudcode-pa.googleapis.com/v1internal?project=p&antigravity=true",
            "gemini-3-pro",
            json!({"contents":[]}),
            true,
        )
        .unwrap();
        assert_eq!(
            url,
            "https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse"
        );
        assert_eq!(body["project"], "p");
        assert_eq!(body["requestType"], "agent");
        assert_eq!(body["request"]["contents"], json!([]));
        assert!(
            prepare_request(
                "https://cloudcode-pa.googleapis.com/v1internal",
                "m",
                json!({}),
                true
            )
            .is_err()
        );
        assert!(
            prepare_request(
                "https://example.com/v1beta?key=secret",
                "m",
                json!({}),
                true
            )
            .is_err()
        );
        assert_eq!(
            models_endpoint("https://example.com/v1beta/models?project=p"),
            "https://example.com/v1beta/models"
        );
    }

    #[test]
    fn google_registry_aliases_serialize_to_one_wire() {
        use crate::llm::provider::{ProviderKind, ProviderRef, ProviderRegistry, WireFamily};
        for alias in [
            "google-generate-content",
            "gemini",
            "google",
            "antigravity",
            "agy",
        ] {
            assert_eq!(
                WireFamily::parse(alias),
                Some(WireFamily::GoogleGenerateContent)
            );
            assert_eq!(
                serde_json::from_value::<WireFamily>(json!(alias)).unwrap(),
                WireFamily::GoogleGenerateContent
            );
        }
        let provider = ProviderRegistry::default()
            .resolve(&ProviderRef::parse("gemini").unwrap())
            .unwrap();
        assert_eq!(
            provider.reference.vendor(),
            Some(ProviderKind::GoogleGemini)
        );
        assert_eq!(provider.endpoint, ENDPOINT);
        assert_eq!(
            serde_json::to_string(&provider.wire).unwrap(),
            "\"google-generate-content\""
        );
    }
}
