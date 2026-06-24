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
        /// True only when the provider terminated the response on a clean
        /// terminal signal (`[DONE]`, `response.completed`, or a
        /// `response.done` whose status is `completed`). A synthetic
        /// ResponseDone emitted after an EOF/timeout, or one derived from a
        /// `failed`/`incomplete`/`cancelled` terminal, carries `clean=false`.
        ///
        /// Downstream uses this to decide whether the chain
        /// (`previous_response_id`) may advance (clean) or must be reset so the
        /// next turn replays from local history (dirty).
        clean: bool,
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
