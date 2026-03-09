use anyhow::{Context, Result};
use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent};
use serde_json::{Value, json};

pub fn register(registry: &mut ToolRegistry) {
    registry
        .register(
            type_text_definition(),
            Box::new(|input| Box::pin(handle_type_text(input))),
        )
        .expect("register type_text tool");
}

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

async fn handle_type_text(input: Value) -> Vec<ToolResultContent> {
    match type_text_from_input(&input) {
        Ok(content) => vec![content],
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

fn type_text_from_input(input: &Value) -> Result<ToolResultContent> {
    let text = input
        .get("text")
        .and_then(Value::as_str)
        .context("Missing required string field 'text'")?;

    crate::os::clipboard::paste_and_restore(text).context("Failed to type text via clipboard")?;
    Ok(ToolResultContent::Text("ok".to_string()))
}
