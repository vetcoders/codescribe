//! OpenAI / Codex-backend JSON Schema subset adapter for tool parameters.
//!
//! MCP servers (Porkbun 0.22.2 among them) publish JSON Schema produced by
//! Zod `.email()`, whose `pattern` contains regex lookaround. The OpenAI
//! Responses / Codex-backend validator rejects that subset with HTTP 400:
//! `regex lookaround is not supported`.
//!
//! Compatibility policy (provider boundary only):
//!
//! - Recurse only through JSON Schema schema-bearing keywords. `default`,
//!   `examples`, `example`, `const`, and `enum` are payload data and are never
//!   walked as schemas.
//! - Supported constraints (`type`, `required`, `properties`, `enum`,
//!   `format`, length/numeric bounds, `additionalProperties` bools, …) stay.
//! - A `pattern` (or `patternProperties` key) containing lookaround is dropped
//!   from the *provider copy*. It is not rewritten into a pretend-equivalent
//!   regex. Unknown lookaround is not mapped onto an invented `format`.
//! - The well-known Zod `.email()` lookaround pattern may receive
//!   `format: "email"` when format is absent: that is OpenAI-representable and
//!   matches the original constraint's intent. Provenance records the action.
//! - The registry / MCP-upstream schema is not mutated. Upstream
//!   `validateToolInput` still sees the original lookaround pattern.
//! - Adaptation provenance is json-pointer + kind + action. Pattern text and
//!   argument values are never logged.

use serde_json::Value;

/// Zod 4 / Porkbun MCP 0.22.2 `z.string().email()` JSON Schema `pattern`.
///
/// Lookahead forbids a leading dot and consecutive dots; OpenAI rejects it.
pub(crate) const ZOD_EMAIL_LOOKAROUND_PATTERN: &str = r"^(?!\.)(?!.*\.\.)([A-Za-z0-9_'+\-\.]*)[A-Za-z0-9_+-]@([A-Za-z0-9][A-Za-z0-9\-]*\.)+[A-Za-z]{2,}$";

/// Object maps whose values are schemas.
const SCHEMA_OBJECT_MAPS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "definitions",
    "dependentSchemas",
];

/// Keywords whose value is a single schema (or a boolean schema).
const SINGLE_SCHEMA_KEYS: &[&str] = &[
    "additionalProperties",
    "additionalItems",
    "contains",
    "not",
    "if",
    "then",
    "else",
    "propertyNames",
    "unevaluatedProperties",
    "unevaluatedItems",
    "contentSchema",
];

/// Keywords whose value is an array of schemas.
const SCHEMA_ARRAY_KEYS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

/// One dropped or rewritten constraint at a JSON Pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SchemaChange {
    pub pointer: String,
    pub kind: SchemaChangeKind,
    pub action: SchemaChangeAction,
}

/// Why the provider copy had to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaChangeKind {
    RegexLookaround,
}

impl SchemaChangeKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RegexLookaround => "regex_lookaround",
        }
    }
}

/// What was done instead of sending the unsupported constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaChangeAction {
    PatternRemoved,
    FormatEmailSubstituted,
    PatternPropertiesKeyRemoved,
}

impl SchemaChangeAction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::PatternRemoved => "dropped_pattern",
            Self::FormatEmailSubstituted => "dropped_pattern_set_format_email",
            Self::PatternPropertiesKeyRemoved => "dropped_pattern_properties_key",
        }
    }
}

/// Adapted schema plus provenance. The input value is never mutated.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AdaptedSchema {
    pub schema: Value,
    pub changes: Vec<SchemaChange>,
}

/// Clone `schema` and drop OpenAI-unsupported regex lookaround in schema positions.
#[must_use]
pub(crate) fn adapt_json_schema_for_openai(schema: &Value) -> AdaptedSchema {
    let mut adapted = schema.clone();
    let mut changes = Vec::new();
    walk_schema(&mut adapted, "", &mut changes);
    AdaptedSchema {
        schema: adapted,
        changes,
    }
}

fn walk_schema(value: &mut Value, pointer: &str, changes: &mut Vec<SchemaChange>) {
    match value {
        Value::Object(_) => walk_schema_object(value, pointer, changes),
        Value::Array(items) => {
            // Only reached for schema arrays (`allOf` / `prefixItems` / tuple `items`).
            for (index, item) in items.iter_mut().enumerate() {
                walk_schema(item, &format!("{pointer}/{index}"), changes);
            }
        }
        Value::Bool(_) | Value::Null | Value::Number(_) | Value::String(_) => {}
    }
}

fn walk_schema_object(value: &mut Value, pointer: &str, changes: &mut Vec<SchemaChange>) {
    let Some(object) = value.as_object_mut() else {
        return;
    };

    if let Some(pattern) = object
        .get("pattern")
        .and_then(Value::as_str)
        .map(str::to_owned)
        && regex_has_lookaround(&pattern)
    {
        object.remove("pattern");
        let action = if pattern == ZOD_EMAIL_LOOKAROUND_PATTERN
            && object.get("format").and_then(Value::as_str).is_none()
        {
            object.insert("format".to_string(), Value::String("email".to_string()));
            SchemaChangeAction::FormatEmailSubstituted
        } else {
            SchemaChangeAction::PatternRemoved
        };
        changes.push(SchemaChange {
            pointer: child_pointer(pointer, "pattern"),
            kind: SchemaChangeKind::RegexLookaround,
            action,
        });
    }

    if let Some(Value::Object(map)) = object.get_mut("patternProperties") {
        let lookaround_keys: Vec<String> = map
            .keys()
            .filter(|key| regex_has_lookaround(key))
            .cloned()
            .collect();
        for key in lookaround_keys {
            map.remove(&key);
            changes.push(SchemaChange {
                pointer: child_pointer(&child_pointer(pointer, "patternProperties"), &key),
                kind: SchemaChangeKind::RegexLookaround,
                action: SchemaChangeAction::PatternPropertiesKeyRemoved,
            });
        }
    }

    for key in SCHEMA_OBJECT_MAPS {
        let Some(Value::Object(_)) = object.get(*key) else {
            continue;
        };
        let map_pointer = child_pointer(pointer, key);
        let child_keys: Vec<String> = object
            .get(*key)
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        for child_key in child_keys {
            let child_pointer = child_pointer(&map_pointer, &child_key);
            if let Some(child) = object
                .get_mut(*key)
                .and_then(Value::as_object_mut)
                .and_then(|map| map.get_mut(&child_key))
            {
                walk_schema(child, &child_pointer, changes);
            }
        }
    }

    for key in SINGLE_SCHEMA_KEYS {
        match object.get(*key) {
            Some(Value::Bool(_) | Value::Null) | None => {}
            Some(_) => {
                let child_pointer = child_pointer(pointer, key);
                if let Some(child) = object.get_mut(*key) {
                    walk_schema(child, &child_pointer, changes);
                }
            }
        }
    }

    match object.get("items") {
        Some(Value::Bool(_) | Value::Null) | None => {}
        Some(_) => {
            let items_pointer = child_pointer(pointer, "items");
            if let Some(child) = object.get_mut("items") {
                walk_schema(child, &items_pointer, changes);
            }
        }
    }

    for key in SCHEMA_ARRAY_KEYS {
        if matches!(object.get(*key), Some(Value::Array(_))) {
            let child_pointer = child_pointer(pointer, key);
            if let Some(child) = object.get_mut(*key) {
                walk_schema(child, &child_pointer, changes);
            }
        }
    }
}

fn child_pointer(parent: &str, segment: &str) -> String {
    format!("{parent}/{}", pointer_escape(segment))
}

fn pointer_escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// True when `pattern` uses a lookaround group OpenAI's regex subset rejects.
fn regex_has_lookaround(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut index = 0;
    while index + 2 < bytes.len() {
        if bytes[index] == b'(' && bytes[index + 1] == b'?' {
            match bytes[index + 2] {
                b'=' | b'!' => return true,
                b'<' if index + 3 < bytes.len() && matches!(bytes[index + 3], b'=' | b'!') => {
                    return true;
                }
                _ => {}
            }
        }
        index += 1;
    }
    false
}

/// Nested Porkbun `update_contacts` input schema: one `contact` plus
/// `contacts.{registrant,admin,tech,billing}`, each with the Zod email pattern.
#[cfg(test)]
pub(crate) fn porkbun_update_contacts_input_schema() -> Value {
    use serde_json::json;
    let contact = porkbun_contact_object();
    json!({
        "type": "object",
        "properties": {
            "domain": {
                "type": "string",
                "minLength": 3,
                "description": "Domain to edit, e.g. `example.com`",
                "examples": ["(?!lookaround-in-examples)@example.com"]
            },
            "contacts": {
                "type": "object",
                "description": "Per-role contacts; include only the roles you want to change.",
                "properties": {
                    "registrant": contact.clone(),
                    "admin": contact.clone(),
                    "tech": contact.clone(),
                    "billing": contact.clone()
                },
                "additionalProperties": false
            },
            "contact": contact,
            "dry_run": {
                "type": "boolean",
                "description": "If true, validate only — returns wouldSucceed without applying the change."
            },
            "address_validation_choice": {
                "type": "string",
                "enum": ["accept_suggestion", "use_as_entered"]
            }
        },
        "required": ["domain"],
        "additionalProperties": false
    })
}

#[cfg(test)]
fn porkbun_contact_object() -> Value {
    use serde_json::json;
    json!({
        "type": "object",
        "properties": {
            "firstName": {
                "type": "string",
                "description": "Given name. Required for a provided role."
            },
            "country": {
                "type": "string",
                "minLength": 2,
                "maxLength": 2,
                "description": "ISO 3166-1 alpha-2 country code, e.g. `US`, `GB`. Required."
            },
            "email": {
                "type": "string",
                "description": "Contact email. Required.",
                "pattern": ZOD_EMAIL_LOOKAROUND_PATTERN
            }
        },
        "required": ["firstName", "country", "email"],
        "additionalProperties": false
    })
}

#[cfg(test)]
const PORKBUN_EMAIL_POINTERS: &[&str] = &[
    "/properties/contact/properties/email/pattern",
    "/properties/contacts/properties/registrant/properties/email/pattern",
    "/properties/contacts/properties/admin/properties/email/pattern",
    "/properties/contacts/properties/tech/properties/email/pattern",
    "/properties/contacts/properties/billing/properties/email/pattern",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn porkbun_nested_update_contacts_adapts_all_five_email_patterns() {
        let original = porkbun_update_contacts_input_schema();
        for pointer in PORKBUN_EMAIL_POINTERS {
            assert_eq!(
                original.pointer(pointer).and_then(Value::as_str),
                Some(ZOD_EMAIL_LOOKAROUND_PATTERN),
                "fixture must carry the Zod email lookaround at {pointer}"
            );
        }

        let adapted = adapt_json_schema_for_openai(&original);
        assert_eq!(original, porkbun_update_contacts_input_schema());
        assert_eq!(adapted.changes.len(), 5);
        let mut pointers: Vec<&str> = adapted
            .changes
            .iter()
            .map(|change| change.pointer.as_str())
            .collect();
        pointers.sort_unstable();
        let mut expected = PORKBUN_EMAIL_POINTERS.to_vec();
        expected.sort_unstable();
        assert_eq!(pointers, expected);

        for change in &adapted.changes {
            assert_eq!(change.kind, SchemaChangeKind::RegexLookaround);
            assert_eq!(change.action, SchemaChangeAction::FormatEmailSubstituted);
        }

        for pointer in PORKBUN_EMAIL_POINTERS {
            assert!(
                adapted.schema.pointer(pointer).is_none(),
                "adapted schema must drop lookaround pattern at {pointer}"
            );
            let format_pointer = pointer.trim_end_matches("pattern").to_string() + "format";
            assert_eq!(
                adapted
                    .schema
                    .pointer(&format_pointer)
                    .and_then(Value::as_str),
                Some("email"),
                "Zod email lookaround maps to format=email at {format_pointer}"
            );
        }

        assert_eq!(
            adapted.schema.pointer("/properties/domain/minLength"),
            Some(&json!(3))
        );
        assert_eq!(
            adapted
                .schema
                .pointer("/properties/contact/properties/country/minLength"),
            Some(&json!(2))
        );
        assert_eq!(
            adapted
                .schema
                .pointer("/properties/contact/properties/country/maxLength"),
            Some(&json!(2))
        );
        assert_eq!(
            adapted
                .schema
                .pointer("/properties/address_validation_choice/enum"),
            Some(&json!(["accept_suggestion", "use_as_entered"]))
        );
        assert_eq!(
            adapted.schema.pointer("/additionalProperties"),
            Some(&json!(false))
        );
        assert_eq!(
            adapted.schema.pointer("/properties/domain/examples"),
            original.pointer("/properties/domain/examples"),
            "ordinary example payload data is not a schema position"
        );
    }

    #[test]
    fn supported_patterns_and_ordinary_payload_data_are_preserved() {
        let original = json!({
            "type": "object",
            "properties": {
                "host": {
                    "type": "string",
                    "pattern": r"^[a-z0-9.-]+\.[a-z]{2,}$",
                    "minLength": 4,
                    "default": "(?!not-a-schema)@example.com",
                    "examples": ["(?!lookaround-in-data)", "u@example.com"]
                },
                "note": {
                    "type": "string",
                    "pattern": r"^ok$",
                    "const": "(?=still-data)"
                }
            },
            "example": {
                "properties": {
                    "email": {
                        "type": "string",
                        "pattern": ZOD_EMAIL_LOOKAROUND_PATTERN
                    }
                }
            },
            "enum": [r"^(?!admin).*$"],
            "required": ["host"]
        });
        let adapted = adapt_json_schema_for_openai(&original);
        assert!(adapted.changes.is_empty(), "{:?}", adapted.changes);
        assert_eq!(adapted.schema, original);
    }

    #[test]
    fn unknown_lookaround_is_dropped_without_inventing_format() {
        let original = json!({
            "type": "object",
            "properties": {
                "username": {
                    "type": "string",
                    "pattern": r"^(?!admin).+$",
                    "minLength": 1
                }
            }
        });
        let adapted = adapt_json_schema_for_openai(&original);
        assert_eq!(adapted.changes.len(), 1);
        assert_eq!(adapted.changes[0].pointer, "/properties/username/pattern");
        assert_eq!(
            adapted.changes[0].action,
            SchemaChangeAction::PatternRemoved
        );
        assert!(
            adapted
                .schema
                .pointer("/properties/username/format")
                .is_none()
        );
        assert_eq!(
            adapted.schema.pointer("/properties/username/minLength"),
            Some(&json!(1))
        );
        assert_eq!(
            original
                .pointer("/properties/username/pattern")
                .and_then(Value::as_str),
            Some(r"^(?!admin).+$")
        );
    }

    #[test]
    fn defs_and_tuple_items_are_schema_positions() {
        let original = json!({
            "$defs": {
                "email": {
                    "type": "string",
                    "pattern": ZOD_EMAIL_LOOKAROUND_PATTERN
                }
            },
            "allOf": [
                { "type": "object" },
                {
                    "properties": {
                        "contact": { "$ref": "#/$defs/email" }
                    }
                }
            ],
            "prefixItems": [
                { "type": "string", "pattern": r"^(?!x).+$" }
            ],
            "items": [
                { "type": "string", "pattern": r"^plain$" }
            ]
        });
        let adapted = adapt_json_schema_for_openai(&original);
        assert_eq!(
            adapted
                .schema
                .pointer("/$defs/email/format")
                .and_then(Value::as_str),
            Some("email")
        );
        assert!(adapted.schema.pointer("/$defs/email/pattern").is_none());
        assert_eq!(
            adapted
                .schema
                .pointer("/allOf/1/properties/contact/$ref")
                .and_then(Value::as_str),
            Some("#/$defs/email")
        );
        assert!(adapted.schema.pointer("/prefixItems/0/pattern").is_none());
        assert_eq!(
            adapted
                .schema
                .pointer("/items/0/pattern")
                .and_then(Value::as_str),
            Some(r"^plain$")
        );
        assert_eq!(
            original,
            json!({
                "$defs": {
                    "email": {
                        "type": "string",
                        "pattern": ZOD_EMAIL_LOOKAROUND_PATTERN
                    }
                },
                "allOf": [
                    { "type": "object" },
                    {
                        "properties": {
                            "contact": { "$ref": "#/$defs/email" }
                        }
                    }
                ],
                "prefixItems": [
                    { "type": "string", "pattern": r"^(?!x).+$" }
                ],
                "items": [
                    { "type": "string", "pattern": r"^plain$" }
                ]
            })
        );
    }

    #[test]
    fn existing_format_is_kept_when_lookaround_pattern_is_dropped() {
        let original = json!({
            "type": "string",
            "format": "email",
            "pattern": ZOD_EMAIL_LOOKAROUND_PATTERN
        });
        let adapted = adapt_json_schema_for_openai(&original);
        assert_eq!(
            adapted.changes[0].action,
            SchemaChangeAction::PatternRemoved
        );
        assert_eq!(
            adapted.schema.pointer("/format").and_then(Value::as_str),
            Some("email")
        );
        assert!(adapted.schema.pointer("/pattern").is_none());
    }
}
