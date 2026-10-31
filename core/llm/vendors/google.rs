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

    let mut constraint_notes = Vec::new();
    for key in [
        "pattern",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "uniqueItems",
    ] {
        if let Some(val) = source.remove(key) {
            let rendered = match &val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            constraint_notes.push(format!("{key}: {rendered}"));
        }
    }

    if let Some(format_val) = source.remove("format")
        && let Some(fmt) = format_val.as_str()
    {
        if matches!(
            fmt,
            "enum" | "date-time" | "int32" | "int64" | "float" | "double"
        ) {
            source.insert("format".into(), Value::String(fmt.to_string()));
        } else {
            constraint_notes.push(format!("format: {fmt}"));
        }
    }

    if let Some(enum_val) = source.remove("enum") {
        let Value::Array(values) = enum_val else {
            return Err("Google tool schema enum must be an array".into());
        };
        let mut strings = Vec::new();
        let mut saw_null = false;
        for item in values {
            match item {
                Value::Null => saw_null = true,
                Value::String(s) => strings.push(Value::String(s)),
                _ => {
                    return Err(
                        "Google tool schema enum supports only string or null values".into(),
                    );
                }
            }
        }
        if strings.is_empty() {
            return Err("Google tool schema enum must contain at least one string value".into());
        }
        if !schema_type_allows_string(source.get("type")) {
            return Err("Google tool schema enum requires string type".into());
        }
        if saw_null {
            source.insert("nullable".into(), json!(true));
        }
        source
            .entry("type".to_string())
            .or_insert_with(|| Value::String("string".to_string()));
        source.insert("enum".into(), Value::Array(strings));
    }

    for key in [
        "$schema",
        "$id",
        "$defs",
        "definitions",
        "$anchor",
        "$dynamicRef",
        "$dynamicAnchor",
        "$vocabulary",
        "$comment",
        "additionalProperties",
        "patternProperties",
        "unevaluatedProperties",
        "propertyNames",
        "dependentRequired",
        "dependentSchemas",
        "dependencies",
        "additionalItems",
        "unevaluatedItems",
        "contains",
        "minContains",
        "maxContains",
        "prefixItems",
        "minProperties",
        "maxProperties",
        "if",
        "then",
        "else",
        "not",
        "contentEncoding",
        "contentMediaType",
        "contentSchema",
        "deprecated",
        "readOnly",
        "writeOnly",
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
        } else if kind == "null" {
            source.insert("nullable".into(), json!(true));
        } else {
            source.insert("type".into(), kind);
        }
    }
    if let Some(Value::Object(properties)) = source.get_mut("properties") {
        for property in properties.values_mut() {
            *property = normalize_schema_node(property, root, refs, depth + 1)?;
        }
    }
    if let Some(valid_keys) =
        source
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| {
                properties
                    .keys()
                    .cloned()
                    .collect::<std::collections::HashSet<String>>()
            })
        && let Some(Value::Array(required)) = source.get_mut("required")
    {
        required.retain(|entry| entry.as_str().is_some_and(|name| valid_keys.contains(name)));
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
    if let Some(Value::Array(variants)) = source.remove("anyOf") {
        let mut non_null = Vec::new();
        let mut saw_null = false;
        for variant in variants {
            if is_null_schema_branch(&variant)? {
                saw_null = true;
            } else {
                non_null.push(normalize_schema_node(&variant, root, refs, depth + 1)?);
            }
        }
        if saw_null {
            source.insert("nullable".into(), json!(true));
        }
        if non_null.len() == 1 {
            if let Some(only_obj) = non_null.pop().and_then(|v| v.as_object().cloned()) {
                for (k, v) in only_obj {
                    source.entry(k).or_insert(v);
                }
            }
        } else if !non_null.is_empty() {
            source.insert("anyOf".into(), Value::Array(non_null));
        }
    }
    // A const narrows all other constraints. Apply it after nullable unions
    // have been lowered so they cannot reintroduce null beside a fixed string.
    if let Some(const_val) = source.remove("const") {
        let Value::String(value) = const_val else {
            return Err("Google tool schema cannot represent this standalone const".into());
        };
        if !schema_type_allows_string(source.get("type")) {
            return Err("Google tool schema const requires string type".into());
        }
        if let Some(Value::Array(values)) = source.get("enum")
            && !values
                .iter()
                .any(|entry| entry.as_str() == Some(value.as_str()))
        {
            return Err("Google tool schema const conflicts with enum".into());
        }
        source.insert("type".into(), json!("string"));
        source.insert("enum".into(), json!([value]));
        source.remove("nullable");
    }
    if !constraint_notes.is_empty() {
        let suffix = format!("({})", constraint_notes.join("; "));
        match source.get("description").and_then(Value::as_str) {
            Some(existing) if !existing.trim().is_empty() => {
                source.insert(
                    "description".into(),
                    Value::String(format!("{} {suffix}", existing.trim())),
                );
            }
            _ => {
                source.insert("description".into(), Value::String(suffix));
            }
        }
    }
    Ok(Value::Object(source))
}

fn schema_type_allows_string(kind: Option<&Value>) -> bool {
    match kind {
        None => true,
        Some(Value::String(kind)) => kind == "string",
        Some(Value::Array(kinds)) => {
            kinds.iter().any(|kind| kind == "string")
                && kinds
                    .iter()
                    .all(|kind| matches!(kind.as_str(), Some("string" | "null")))
        }
        Some(_) => false,
    }
}

fn is_null_schema_branch(branch: &Value) -> Result<bool, String> {
    let Some(obj) = branch.as_object() else {
        return Ok(false);
    };
    let only_null_type = |value: &Value| {
        value == "null"
            || value
                .as_array()
                .is_some_and(|types| !types.is_empty() && types.iter().all(|kind| kind == "null"))
    };
    let only_null_enum = |value: &Value| {
        value
            .as_array()
            .is_some_and(|values| !values.is_empty() && values.iter().all(Value::is_null))
    };
    let marked_null = obj.get("type").is_some_and(only_null_type)
        || matches!(obj.get("const"), Some(Value::Null))
        || obj.get("enum").is_some_and(only_null_enum);
    if !marked_null {
        return Ok(false);
    }
    for (key, value) in obj {
        let representable = match key.as_str() {
            "type" => only_null_type(value),
            "const" => value.is_null(),
            "enum" => only_null_enum(value),
            "$schema" | "$id" | "$comment" | "title" | "description" | "default" | "examples"
            | "deprecated" | "readOnly" | "writeOnly" => true,
            _ => false,
        };
        if !representable {
            return Err(format!(
                "Google null schema branch has conflicting or unsupported constraint: {key}"
            ));
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_schema_string_const_never_retains_null_from_other_constraints() {
        for schema in [
            json!({"enum":["x",null],"const":"x"}),
            json!({"type":["string","null"],"const":"x"}),
            json!({"anyOf":[{"type":"string"},{"type":"null"}],"const":"x"}),
        ] {
            assert_eq!(
                normalize_schema_for_gemini(&schema).unwrap(),
                json!({"type":"string","enum":["x"]}),
                "a const must not admit another value: {schema}"
            );
        }
    }

    #[test]
    fn google_schema_null_branches_never_hide_assertions() {
        for branch in [
            json!({"enum":[null],"allOf":[{"const":1}]}),
            json!({"enum":[null],"type":"string"}),
            json!({"type":"null","const":"x"}),
            json!({"const":null,"enum":["x"]}),
        ] {
            let schema = json!({"anyOf":[{"type":"string"},branch]});
            assert!(normalize_schema_for_gemini(&schema).is_err(), "{schema}");
        }
        assert_eq!(
            normalize_schema_for_gemini(&json!({
                "anyOf":[{"type":"string","enum":["x"]},{"enum":[null],"description":"optional"}]
            }))
            .unwrap(),
            json!({"type":"string","enum":["x"],"nullable":true})
        );
    }

    #[test]
    fn google_schema_unrepresentable_constants_and_enums_are_refused() {
        for schema in [
            json!({"type":"integer","enum":[1,2],"const":1}),
            json!({"enum":["x",1]}),
            json!({"enum":[]}),
            json!({"type":"string","enum":["x",null],"const":null}),
            json!({"const":null}),
        ] {
            assert!(normalize_schema_for_gemini(&schema).is_err(), "{schema}");
        }
    }

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

        let rich_schema = json!({
            "type": "object",
            "required": ["mode", "ghost", "query"],
            "properties": {
                "mode": {
                    "oneOf": [
                        {"const": "fast"},
                        {"type": "null"}
                    ]
                },
                "query": {
                    "type": "string",
                    "format": "uri",
                    "minLength": 3,
                    "description": "Target URL"
                }
            }
        });
        let rich_norm = normalize_schema_for_gemini(&rich_schema).unwrap();
        assert_eq!(rich_norm["required"], json!(["mode", "query"]));
        assert_eq!(
            rich_norm["properties"]["mode"],
            json!({"type": "string", "enum": ["fast"], "nullable": true})
        );
        assert_eq!(
            rich_norm["properties"]["query"],
            json!({"type": "string", "description": "Target URL (minLength: 3; format: uri)"})
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
