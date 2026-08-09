//! Read-only tools that let the agent see the user's current desktop context:
//! the text selected in the frontmost app, and which app that is.
//!
//! Both observe rather than act, so they register as [`ToolRisk::ReadOnly`] and
//! answer with a readable placeholder instead of an error when there is nothing
//! to report — an empty selection is a normal state, not a failure.

use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent, ToolRisk};
use serde_json::{Value, json};

/// Register both selection-context tools on the registry.
pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_native(
            get_selected_text_definition(),
            Box::new(|input| Box::pin(handle_get_selected_text(input))),
            ToolRisk::ReadOnly,
        )
        .expect("register get_selected_text tool");
    registry
        .register_native(
            get_frontmost_app_definition(),
            Box::new(|input| Box::pin(handle_get_frontmost_app(input))),
            ToolRisk::ReadOnly,
        )
        .expect("register get_frontmost_app tool");
}

/// Schema for `get_selected_text`: no inputs, reads the live selection.
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

/// Schema for `get_frontmost_app`: no inputs, reads the active app name.
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

/// Capture the assistive context and return its selection. A blank or
/// whitespace-only selection reports "No text selected" rather than an error.
async fn handle_get_selected_text(_input: Value) -> Vec<ToolResultContent> {
    let context = crate::os::selection::capture_assistive_context();
    let selected_text = context
        .selected_text
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| "No text selected".to_string());

    vec![ToolResultContent::Text(selected_text)]
}

/// Report the frontmost application's name, falling back to "Unknown" when
/// the accessibility query yields nothing usable.
async fn handle_get_frontmost_app(_input: Value) -> Vec<ToolResultContent> {
    let context = crate::os::selection::capture_frontmost_app_only();
    let frontmost_app = context
        .frontmost_app
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Unknown".to_string());

    vec![ToolResultContent::Text(frontmost_app)]
}
