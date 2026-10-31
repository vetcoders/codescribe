//! ACP v1 JSON-RPC envelopes. Stdio carries exactly one envelope per line.

use serde_json::{Value, json};

use super::AcpError;
use crate::agent::redaction::{REDACTED, redact_command_line, redacted_arguments};

/// ACP's supported major protocol version.
pub const PROTOCOL_VERSION: u64 = 1;

/// Construct a client request.
pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// Construct a notification, which never carries an id.
pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// Answer an incoming request using its original (string or numeric) id.
pub fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Answer a rejected/unsupported request without exposing raw arguments.
pub fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": redact_command_line(message)}})
}

/// Redact before truncating, otherwise the cut could sever a secret marker.
pub fn diagnostic_prefix(text: &str) -> String {
    let redacted = redact_diagnostic(text);
    let mut end = redacted.len().min(120);
    while !redacted.is_char_boundary(end) {
        end -= 1;
    }
    redacted[..end].to_string()
}

/// Diagnostic-only redaction, including JSON that was cut or malformed.
pub(crate) fn redact_diagnostic(text: &str) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => redacted_arguments(&value).unwrap_or_default(),
        Err(_) => {
            // A broken JSON object's key/value structure cannot be redacted
            // reliably as shell tokens (a benign first key can hide a later
            // secret key inside one compact token). Keep only its banner.
            match text.find(['{', '[']) {
                Some(start) => format!("{}{REDACTED}", redact_command_line(&text[..start])),
                None => redact_command_line(text),
            }
        }
    }
}

/// Parse one strict JSON-RPC object, never accepting banners or JSON scalars.
pub fn parse_frame(bytes: &[u8]) -> Result<Value, AcpError> {
    let malformed = || AcpError::MalformedFrame(diagnostic_prefix(&String::from_utf8_lossy(bytes)));
    let frame: Value = serde_json::from_slice(bytes).map_err(|_| malformed())?;
    let object = frame.as_object().ok_or_else(malformed)?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(malformed());
    }
    let id = object.get("id");
    if id.is_some_and(|id| !id.is_string() && !id.is_number() && !id.is_null()) {
        return Err(malformed());
    }
    // IDs are echoed on cancellation as well as ordinary replies. Keep that
    // bounded independently of a frame's much larger content allowance.
    if id.and_then(Value::as_str).is_some_and(|id| id.len() > 1024) {
        return Err(malformed());
    }
    if let Some(method) = object.get("method") {
        if !method.as_str().is_some_and(|method| !method.is_empty())
            || object.contains_key("result")
            || object.contains_key("error")
            || object
                .get("params")
                .is_some_and(|params| !params.is_object() && !params.is_array())
        {
            return Err(malformed());
        }
    } else {
        if id.is_none() || object.contains_key("result") == object.contains_key("error") {
            return Err(malformed());
        }
        if let Some(error) = object.get("error")
            && (error.get("code").and_then(Value::as_i64).is_none()
                || error.get("message").and_then(Value::as_str).is_none())
        {
            return Err(malformed());
        }
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_envelopes_reject_banners_scalars_and_ambiguous_responses() {
        for frame in [
            "hello",
            "[]",
            "true",
            "{}",
            r#"{"jsonrpc":"2.0","id":1,"result":{},"error":{"code":-1,"message":"x"}}"#,
            r#"{"jsonrpc":"2.0","method":"x","params":false}"#,
        ] {
            assert!(matches!(
                parse_frame(frame.as_bytes()),
                Err(AcpError::MalformedFrame(_))
            ));
        }
        assert!(
            parse_frame(&serde_json::to_vec(&request(1, "initialize", json!({}))).unwrap()).is_ok()
        );
        assert!(
            parse_frame(&serde_json::to_vec(&success(json!("agent-1"), json!({}))).unwrap())
                .is_ok()
        );
    }

    #[test]
    fn acp_malformed_diagnostics_redact_then_bound_on_utf8() {
        let line = format!("TOKEN=private-value {}", "ż".repeat(150));
        let diagnostic = diagnostic_prefix(&line);
        assert!(!diagnostic.contains("private-value"));
        assert!(diagnostic.len() <= 120);
        assert!(diagnostic.contains("[REDACTED]"));
        let compact_broken = r#"banner {"level":"error","token":"private-value"#;
        assert_eq!(diagnostic_prefix(compact_broken), "banner [REDACTED]");
    }
}
