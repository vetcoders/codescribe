//! Agent streaming surface — thin UniFFI wrapper over the live codescribe
//! `AgentSession` (token/reasoning/tool-call streaming). Moved out of `lib.rs`
//! in W3 cut #0 so each bridge slice owns a disjoint file.

use std::sync::Arc;

use codescribe_core::agent::{
    AgentSession, AgentUiEvent, ContentBlock, Message, Role, StreamOptions, Thread, ThreadMessage,
    ThreadStore, ToolRegistry,
};

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
#[derive(uniffi::Object, Default)]
pub struct CodescribeAgent {}

#[uniffi::export(async_runtime = "tokio")]
impl CodescribeAgent {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self::default()
    }

    /// True when the assistive LLM provider can be built from the environment
    /// (LLM_ASSISTIVE_ENDPOINT / _MODEL / _API_KEY present). Same gate the live
    /// app uses before agent replies are possible.
    pub fn is_available(&self) -> bool {
        // Warm settings + Keychain only when the agent surface is actually used.
        // Constructing the Swift app model must not trigger a keychain prompt.
        let _ = codescribe_core::config::Config::load();
        codescribe::agent::create_default_provider().is_ok()
    }

    /// Stream one agent reply for `text` on the conversation identified by
    /// `thread_id`, forwarding token/reasoning/tool events to `listener` as they
    /// arrive. Returns the final assembled assistant text.
    ///
    /// Memory + persistence: prior turns stored under `thread_id` are restored
    /// into the session before sending (so the model sees the conversation
    /// history), and the updated thread is written back after a successful reply
    /// so the SwiftUI app's conversations survive restart. Persistence is
    /// best-effort: a load/save failure never fails the reply the user already
    /// saw.
    ///
    /// Full native tool set + MCP are registered, so the agent can actually act
    /// (clipboard, selection, screenshot, filesystem, typing, github, search,
    /// transcribe). Tools execute on demand when the model calls them.
    pub async fn stream_reply(
        &self,
        text: String,
        thread_id: String,
        listener: Arc<dyn CsAgentListener>,
    ) -> Result<String, CsError> {
        // Keep provider construction behavior identical to the old eager
        // constructor path, but delay it until the user sends a message.
        let _ = codescribe_core::config::Config::load();
        let provider = codescribe::agent::create_default_provider()?;
        let mut registry = ToolRegistry::new();
        codescribe::agent::tools::register_all_tools(&mut registry);
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::channel::<AgentUiEvent>(64);
        let mut session = AgentSession::new(provider, Arc::new(registry), ui_tx);

        // Restore prior turns for cross-turn memory. ThreadStore does blocking
        // fs I/O, so the load runs on a blocking pool thread and is awaited
        // before the agent loop starts. A missing/corrupt thread yields an empty
        // history (best-effort: a first turn simply has nothing to restore).
        let thread_id_for_load = thread_id.clone();
        let restored: Vec<Message> = tokio::task::spawn_blocking(move || {
            ThreadStore::new()
                .ok()
                .and_then(|store| store.load_thread(&thread_id_for_load).ok())
                .map(|thread| {
                    thread
                        .messages
                        .iter()
                        .map(ThreadMessage::to_message)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        if !restored.is_empty() {
            // Seeds the conversation history; resets the provider chain id to
            // None (the persistence id is `thread_id`, separate from the
            // provider's response-chain id).
            session.restore_messages(restored);
        }

        // Drive the agent loop on a task so the channel closes when it finishes,
        // letting the drain loop below terminate cleanly. The task hands back the
        // session's final message log so the caller can persist the thread.
        let send_handle = tokio::spawn(async move {
            let mut session = session;
            let options = StreamOptions {
                model: String::new(),
                system_prompt: None,
                max_tokens: None,
                temperature: None,
                reset_chain: false,
            };
            session.send(text, Vec::new(), &options).await?;
            Ok::<Vec<Message>, anyhow::Error>(session.messages().to_vec())
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
            Ok(Ok(messages)) => {
                // Persist the updated thread (best-effort). The reply already
                // streamed to the user, so a save failure is logged-and-ignored
                // rather than surfaced as an error.
                persist_thread(thread_id, messages).await;
                Ok(final_text)
            }
            Ok(Err(error)) => Err(CsError::Agent {
                msg: error.to_string(),
            }),
            Err(join_error) => Err(CsError::Agent {
                msg: format!("agent task join error: {join_error}"),
            }),
        }
    }
}

/// Persist (create or update) the thread identified by `thread_id` from the
/// session's final `messages`. Mirrors the live app's `persist_runtime_thread`
/// (app/controller/helpers.rs): load-or-build a `Thread`, refresh title/summary/
/// messages, and save. Runs the blocking fs work on a blocking pool thread and
/// swallows any error — persistence is best-effort.
async fn persist_thread(thread_id: String, messages: Vec<Message>) {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let store = ThreadStore::new()?;

        // `now` is sourced from the freshest message timestamp the session
        // stamped (`Some(Utc::now())` per turn), avoiding a direct `chrono`
        // dependency in the bridge crate. With nothing to anchor the thread to,
        // skip the write.
        let Some(now) = messages.iter().rev().find_map(|message| message.timestamp) else {
            return Ok(());
        };

        let model = std::env::var("LLM_ASSISTIVE_MODEL").unwrap_or_default();

        let mut thread = store.load_thread(&thread_id).unwrap_or_else(|_| Thread {
            id: thread_id.clone(),
            created_at: now,
            updated_at: now,
            title: "Codescribe Agent Chat".to_string(),
            mode: "assistive".to_string(),
            tags: vec!["agent".to_string(), "overlay".to_string()],
            notes: Vec::new(),
            messages: Vec::new(),
            summary: None,
            total_tokens: None,
            provider: "openai-responses".to_string(),
            model: model.clone(),
        });

        thread.updated_at = now;
        thread.title = derive_thread_title(&messages);
        thread.summary = derive_thread_summary(&messages);
        thread.messages = messages.iter().map(ThreadMessage::from).collect();
        thread.provider = "openai-responses".to_string();
        thread.model = model;

        store.save_thread(&thread)?;
        Ok(())
    })
    .await;

    if let Ok(Err(error)) = result {
        // Bridge crate has no logging dep; stderr keeps the best-effort failure
        // visible without taking the reply down.
        eprintln!("Failed to persist agent thread (best-effort): {error}");
    }
}

/// First user message, trimmed to a title-length slice. Replica of
/// `derive_thread_title` in app/controller/helpers.rs.
fn derive_thread_title(messages: &[Message]) -> String {
    let candidate = messages
        .iter()
        .find(|message| message.role == Role::User)
        .and_then(extract_text_from_message)
        .unwrap_or_else(|| "Codescribe Agent Chat".to_string());

    let mut title = candidate.chars().take(72).collect::<String>();
    if title.is_empty() {
        title = "Codescribe Agent Chat".to_string();
    }
    title
}

/// Latest assistant message, trimmed to a summary-length slice. Replica of
/// `derive_thread_summary` in app/controller/helpers.rs.
fn derive_thread_summary(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .and_then(extract_text_from_message)
        .map(|text| {
            let mut clipped = text.chars().take(240).collect::<String>();
            if clipped.is_empty() {
                clipped = "Assistant response".to_string();
            }
            clipped
        })
}

/// Flatten a message's textual content into a single normalized string. Replica
/// of `extract_text_from_message` in app/controller/helpers.rs.
fn extract_text_from_message(message: &Message) -> Option<String> {
    let mut out = Vec::new();
    for block in &message.content {
        extract_text_from_block(block, &mut out);
    }
    let text = out.join(" ");
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

/// Collect text from a content block (recursing into tool results). Replica of
/// `extract_text_from_block` in app/controller/helpers.rs.
fn extract_text_from_block(block: &ContentBlock, out: &mut Vec<String>) {
    match block {
        ContentBlock::Text(text) if !text.trim().is_empty() => {
            out.push(text.to_string());
        }
        ContentBlock::ToolResult { content, .. } => {
            for nested in content {
                extract_text_from_block(nested, out);
            }
        }
        _ => {}
    }
}
