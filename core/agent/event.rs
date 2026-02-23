/// Unified provider event stream consumed by the agent loop.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    TextDelta(String),
    TextDone(String),
    ReasoningDelta(String),
    ToolCallStart {
        id: String,
        name: String,
    },
    ToolCallArgsDelta {
        id: String,
        delta: String,
    },
    ToolCallReady {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    ResponseDone {
        response_id: Option<String>,
    },
    Error(String),
}

/// Provider-agnostic UI events.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentUiEvent {
    TextDelta(String),
    TextDone(String),
    ReasoningDelta(String),
    ToolExecuting {
        name: String,
        id: String,
    },
    ToolResult {
        name: String,
        id: String,
        summary: String,
    },
    Done,
    Error(String),
}
