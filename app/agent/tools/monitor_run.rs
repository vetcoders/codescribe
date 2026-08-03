//! `monitor_run` — register a resident monitor for a Vibecrafted run.
//!
//! The tool call is the *registration* only: the application-owned service
//! loop (`crate::agent::monitor`) keeps observing after this turn ends and
//! heartbeats meaningful state changes back into this exact thread. This is
//! the durable replacement for pretending a transient `tail -f` will be
//! revisited.

use codescribe_core::agent::{ToolDefinition, ToolRegistry, ToolResultContent, ToolRisk};
use serde_json::{Value, json};

pub const MONITOR_RUN_TOOL: &str = "monitor_run";

pub fn register(registry: &mut ToolRegistry) {
    registry
        .register_native(
            monitor_run_definition(),
            Box::new(|input| Box::pin(handle_monitor_run(input))),
            // Registration only observes a control-plane run; the side effect
            // is an in-app heartbeat into the caller's own thread.
            ToolRisk::ReadOnly,
        )
        .expect("register monitor_run tool");
    registry.enable_thread_context(MONITOR_RUN_TOOL);
}

fn monitor_run_definition() -> ToolDefinition {
    ToolDefinition {
        name: MONITOR_RUN_TOOL.to_string(),
        description: "Register a resident monitor for a Vibecrafted run (by run_id, e.g. \
                      'work-260801-104012-91713'). Codescribe keeps observing the run after \
                      this conversation turn ends and posts a heartbeat into this thread on \
                      meaningful state changes: stall, block, recovery, completion, failure, \
                      or a missing promised report (needs-rerun). Use this instead of tailing \
                      logs; registration is idempotent per run and thread."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "run_id": {
                    "type": "string",
                    "description": "Canonical Vibecrafted run id from the control plane."
                }
            },
            "required": ["run_id"]
        }),
    }
}

async fn handle_monitor_run(input: Value) -> Vec<ToolResultContent> {
    let Some(run_id) = input
        .get("run_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return vec![ToolResultContent::Error(
            "Missing required non-empty string field 'run_id'".to_string(),
        )];
    };
    // Injected by the session for thread-context tools; absent means the turn
    // has no durable thread to heartbeat into.
    let thread_id = input
        .get("_thread_id")
        .and_then(Value::as_str)
        .unwrap_or("unbound");

    match crate::agent::monitor::register_run_monitor(run_id, thread_id) {
        Ok(record) => {
            let receipt = json!({
                "monitor_id": record.monitor_id,
                "run_id": record.run_id,
                "state": record.state.as_str(),
                "poll_interval_secs": record.poll_interval_secs,
                "message": format!(
                    "Resident monitor registered for run '{}'. I will post a heartbeat into \
                     this thread on stall, recovery, completion, or failure — no further \
                     polling needed in this turn.",
                    record.run_id
                ),
            });
            vec![ToolResultContent::Text(receipt.to_string())]
        }
        Err(error) => vec![ToolResultContent::Error(error.to_string())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_run_id_is_a_readable_error() {
        let result = handle_monitor_run(json!({})).await;
        assert!(matches!(result[0], ToolResultContent::Error(_)));
    }

    #[tokio::test]
    async fn unbound_thread_is_rejected_before_touching_the_store() {
        let result = handle_monitor_run(json!({"run_id": "work-x"})).await;
        let ToolResultContent::Error(message) = &result[0] else {
            panic!("expected error result");
        };
        assert!(
            message.contains("No durable thread") || message.contains("no control-plane meta"),
            "got: {message}"
        );
    }
}
