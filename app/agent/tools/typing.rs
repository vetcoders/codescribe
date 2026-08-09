//! `type_text` — inject text into whatever application currently has focus.
//!
//! Delivery goes through the clipboard (Cmd+V) rather than synthesised
//! per-character keystrokes, which is what makes it reliable across apps with
//! different input handling. The clipboard is restored afterwards.

use anyhow::{Context, Result};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent, ToolRisk};
use serde_json::{Value, json};

/// Register `type_text` as a mutating (approval-gated) tool.
pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_native(
            type_text_definition(),
            Box::new(|input| Box::pin(handle_type_text(input))),
            // Injects keystrokes into whatever app is frontmost — if that is a
            // terminal, this is arbitrary command execution outside every path
            // and program policy. Must ask (review P1-06).
            ToolRisk::Mutating,
        )
        .expect("register type_text tool");
}

/// Schema for `type_text`: a single required `text` string.
fn type_text_definition() -> ToolDefinition {
    ToolDefinition {
        name: "type_text".to_string(),
        description: "Type text into the currently focused application by simulating keyboard input. The text is pasted via clipboard (Cmd+V) for reliability.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to type"
                }
            },
            "required": ["text"]
        }),
    }
}

/// Tool entry point: turn any failure into a readable tool error rather than
/// letting it escape as a panic across the agent loop.
async fn handle_type_text(input: Value) -> Vec<ToolResultContent> {
    match type_text_from_input(&input) {
        Ok(content) => vec![content],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

/// Validate the input and perform the paste, keeping the fallible work out of
/// the async handler so each failure carries its own context.
fn type_text_from_input(input: &Value) -> Result<ToolResultContent> {
    let text = input
        .get("text")
        .and_then(Value::as_str)
        .context("Missing required string field 'text'")?;

    crate::os::clipboard::paste_and_restore(text).context("Failed to type text via clipboard")?;
    Ok(ToolResultContent::Text("ok".to_string()))
}
