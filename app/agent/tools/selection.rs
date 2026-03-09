use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent};
use serde_json::{Value, json};

pub fn register(registry: &mut ToolRegistry) {
    registry
        .register(
            get_selected_text_definition(),
            Box::new(|input| Box::pin(handle_get_selected_text(input))),
        )
        .expect("register get_selected_text tool");
    registry
        .register(
            get_frontmost_app_definition(),
            Box::new(|input| Box::pin(handle_get_frontmost_app(input))),
        )
        .expect("register get_frontmost_app tool");
}

fn get_selected_text_definition() -> ToolDefinition {
    ToolDefinition {
        name: "get_selected_text".to_string(),
        description: "Get the currently selected text in the frontmost application. Uses macOS Accessibility API with Cmd+C fallback for web browsers.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    }
}

fn get_frontmost_app_definition() -> ToolDefinition {
    ToolDefinition {
        name: "get_frontmost_app".to_string(),
        description: "Get the name of the currently active (frontmost) application.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {}
        }),
    }
}

async fn handle_get_selected_text(_input: Value) -> Vec<ToolResultContent> {
    let context = crate::os::selection::capture_assistive_context();
    let selected_text = context
        .selected_text
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| "No text selected".to_string());

    vec![ToolResultContent::Text(selected_text)]
}

async fn handle_get_frontmost_app(_input: Value) -> Vec<ToolResultContent> {
    let context = crate::os::selection::capture_frontmost_app_only();
    let frontmost_app = context
        .frontmost_app
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Unknown".to_string());

    vec![ToolResultContent::Text(frontmost_app)]
}
