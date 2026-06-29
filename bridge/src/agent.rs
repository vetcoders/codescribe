//! Agent streaming surface — thin UniFFI wrapper over the live codescribe
//! `AgentSession` (token/reasoning/tool-call streaming). Moved out of `lib.rs`
//! in W3 cut #0 so each bridge slice owns a disjoint file.

use std::sync::Arc;

use codescribe_core::agent::{AgentSession, AgentUiEvent, StreamOptions, ToolRegistry};

use crate::CsError;

/// Foreign callback trait — agent streaming events forwarded to Swift.
/// Mirrors `AgentUiEvent`; the Swift side must hop these onto the main actor.
#[uniffi::export(with_foreign)]
pub trait CsAgentListener: Send + Sync {
    fn on_text_delta(&self, delta: String);
    fn on_text_done(&self, text: String);
    fn on_reasoning_delta(&self, delta: String);
    fn on_tool_executing(&self, name: String, id: String);
    fn on_tool_result(&self, name: String, id: String, summary: String, is_error: bool);
    fn on_done(&self);
    fn on_error(&self, message: String);
}

/// Thin handle to the codescribe agent engine.
#[derive(uniffi::Object)]
pub struct CodescribeAgent {}

#[uniffi::export(async_runtime = "tokio")]
impl CodescribeAgent {
    #[uniffi::constructor]
    pub fn new() -> Self {
        // Populate the process env (Keychain keys + settings.json + default LLM
        // runtime endpoint/model) exactly like the live app's startup, so the
        // assistive provider can be built. Idempotent; safe to call repeatedly.
        let _ = codescribe_core::config::Config::load();
        Self {}
    }

    /// True when the assistive LLM provider can be built from the environment
    /// (LLM_ASSISTIVE_ENDPOINT / _MODEL / _API_KEY present). Same gate the live
    /// app uses before agent replies are possible.
    pub fn is_available(&self) -> bool {
        codescribe::agent::create_default_provider().is_ok()
    }

    /// Stream one agent reply for `text`, forwarding token/reasoning/tool events to
    /// `listener` as they arrive. Returns the final assembled assistant text.
    ///
    /// Full native tool set + MCP are registered, so the agent can actually act
    /// (clipboard, selection, screenshot, filesystem, typing, github, search,
    /// transcribe). Tools execute on demand when the model calls them.
    pub async fn stream_reply(
        &self,
        text: String,
        listener: Arc<dyn CsAgentListener>,
    ) -> Result<String, CsError> {
        let provider = codescribe::agent::create_default_provider()?;
        let mut registry = ToolRegistry::new();
        codescribe::agent::tools::register_all_tools(&mut registry);
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel::<AgentUiEvent>(64);
        let session = AgentSession::new(provider, Arc::new(registry), ui_tx);

        // Drive the agent loop on a task so the channel closes when it finishes,
        // letting the drain loop below terminate cleanly.
        let send_handle = tokio::spawn(async move {
            let mut session = session;
            let options = StreamOptions {
                model: String::new(),
                system_prompt: None,
                max_tokens: None,
                temperature: None,
                reset_chain: false,
            };
            session.send(text, Vec::new(), &options).await
        });

        let mut final_text = String::new();
        while let Some(event) = ui_rx.recv().await {
            match event {
                AgentUiEvent::TextDelta(delta) => listener.on_text_delta(delta),
                AgentUiEvent::TextDone(t) => {
                    final_text = t.clone();
                    listener.on_text_done(t);
                }
                AgentUiEvent::ReasoningDelta(delta) => listener.on_reasoning_delta(delta),
                AgentUiEvent::ToolExecuting { name, id } => listener.on_tool_executing(name, id),
                AgentUiEvent::ToolResult {
                    name,
                    id,
                    summary,
                    is_error,
                } => listener.on_tool_result(name, id, summary, is_error),
                AgentUiEvent::Done => listener.on_done(),
                AgentUiEvent::Error(message) => listener.on_error(message),
            }
        }

        match send_handle.await {
            Ok(Ok(())) => Ok(final_text),
            Ok(Err(error)) => Err(CsError::Agent {
                msg: error.to_string(),
            }),
            Err(join_error) => Err(CsError::Agent {
                msg: format!("agent task join error: {join_error}"),
            }),
        }
    }
}
