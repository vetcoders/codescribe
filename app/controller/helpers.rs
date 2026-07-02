//! Controller helper functions
//!
//! Session state management and utility functions.

use chrono::Utc;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use tokio::sync::{Mutex as TokioMutex, mpsc};
use tracing::{debug, info, warn};

use crate::config::default_assistive_model;
use anyhow::{Context, Result};
use codescribe_core::agent::{
    AgentSession, AgentUiEvent, ContentBlock, ImageAttachment, Message, Role, StreamOptions,
    Thread, ThreadMessage, ThreadStore, ToolRegistry,
};
use serde_json::json;

/// Global flag for current session mode.
/// true = assistive (chat UI), false = non-assistive (simple transcription overlay)
/// This is set before recording starts and checked by the delta callback.
static IS_ASSISTIVE_SESSION: AtomicBool = AtomicBool::new(false);

/// Global flag for conversation mode (full-duplex Moshi).
/// When true, audio is routed to ConversationEngine instead of Whisper.
static IS_CONVERSATION_SESSION: AtomicBool = AtomicBool::new(false);

/// Set the current session mode (called before recording starts)
pub fn set_assistive_session(is_assistive: bool) {
    IS_ASSISTIVE_SESSION.store(is_assistive, Ordering::SeqCst);
}

/// Check if current session is assistive mode
pub fn is_assistive_session() -> bool {
    IS_ASSISTIVE_SESSION.load(Ordering::SeqCst)
}

/// Set conversation mode flag (Moshi full-duplex)
pub fn set_conversation_session(is_conversation: bool) {
    IS_CONVERSATION_SESSION.store(is_conversation, Ordering::SeqCst);
}

/// Check if current session is conversation mode (Moshi)
pub fn is_conversation_session() -> bool {
    IS_CONVERSATION_SESSION.load(Ordering::SeqCst)
}

/// Route transcription delta to the active overlay.
///
/// Contract:
/// - Assistive sessions stream into Agent overlay chat bubbles.
/// - Non-assistive sessions publish engine events over IPC/FFI for the Swift overlay.
/// - `delta` must already follow `TranscriptDelta` backspace semantics.
///   This function must never receive full preview snapshots.
pub fn route_transcription_delta(_delta: &str) {
    // Legacy AppKit overlay delivery removed. Assistive deltas reach SwiftUI via
    // the engine event broadcast (see IpcBroadcastSink / subscribe_events).
}

/// DeltaSink that routes deltas to the active UI overlay.
///
/// Uses `is_assistive_session()` to decide: chat bubble vs transcription overlay.
/// Plugs into `PresentationEmitter` → `BufferedEmitter` → delta chain.
pub struct RoutingDeltaSink;

impl codescribe_core::pipeline::contracts::DeltaSink for RoutingDeltaSink {
    fn apply(&self, delta: &codescribe_core::pipeline::contracts::TranscriptDelta) {
        route_transcription_delta(&delta.delta);
    }
}

const AGENT_UI_CHANNEL_CAPACITY: usize = 256;
static AGENT_THREAD_GENERATION: AtomicU64 = AtomicU64::new(1);
static AGENT_SEND_IN_FLIGHT_COUNT: AtomicUsize = AtomicUsize::new(0);
static SHARED_AGENT_RUNTIME_STATE: OnceLock<StdMutex<Option<Arc<TokioMutex<AgentRuntimeState>>>>> =
    OnceLock::new();
const CODESCRIBE_ASSISTIVE_LEGACY_BACKUP_ENV: &str =
    "CODESCRIBE_ASSISTIVE_LEGACY_TRANSCRIPT_BACKUP";

struct AgentRuntime {
    session: AgentSession,
    ui_rx: mpsc::Receiver<AgentUiEvent>,
    thread_store_id: String,
}

#[derive(Default)]
struct AgentRuntimeState {
    runtime: Option<AgentRuntime>,
    runtime_generation: u64,
    runtime_degraded: bool,
}

struct AgentSendInFlightGuard;

impl AgentSendInFlightGuard {
    fn new() -> Self {
        AGENT_SEND_IN_FLIGHT_COUNT.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for AgentSendInFlightGuard {
    fn drop(&mut self) {
        AGENT_SEND_IN_FLIGHT_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) fn is_agent_send_in_flight() -> bool {
    AGENT_SEND_IN_FLIGHT_COUNT.load(Ordering::SeqCst) > 0
}

#[cfg(test)]
pub(super) fn set_agent_send_in_flight_for_test(active: bool) {
    AGENT_SEND_IN_FLIGHT_COUNT.store(if active { 1 } else { 0 }, Ordering::SeqCst);
}

impl AgentRuntimeState {
    fn ensure_runtime(&mut self, runtime_generation: u64) -> Result<(&mut AgentRuntime, bool)> {
        self.ensure_runtime_with(runtime_generation, initialize_agent_runtime)
    }

    fn ensure_runtime_with<F>(
        &mut self,
        runtime_generation: u64,
        initialize_runtime: F,
    ) -> Result<(&mut AgentRuntime, bool)>
    where
        F: FnOnce() -> Result<AgentRuntime>,
    {
        let mut recovered_from_degraded = false;
        if self.runtime_generation != runtime_generation {
            self.runtime = None;
            self.runtime_generation = runtime_generation;
        }
        if self.runtime.is_none() {
            self.runtime = Some(initialize_runtime()?);
            if self.runtime_degraded {
                self.runtime_degraded = false;
                recovered_from_degraded = true;
            }
        }
        let runtime = self
            .runtime
            .as_mut()
            .context("Agent runtime was not initialized")?;
        Ok((runtime, recovered_from_degraded))
    }

    /// Hard degrade: the agent runtime is gone (provider unreachable / init
    /// failed). Drops the whole runtime — conversation history is lost. Use only
    /// when the runtime cannot be trusted to hold valid state.
    fn mark_runtime_degraded(&mut self) -> bool {
        self.runtime = None;
        if self.runtime_degraded {
            false
        } else {
            self.runtime_degraded = true;
            true
        }
    }

    /// Soft degrade (P1.7): a transient in-conversation failure that does NOT
    /// invalidate the conversation. Keep the runtime and its `session.messages`
    /// alive, but reset the provider chain (`previous_response_id`) so the next
    /// turn does a full replay from local history instead of resuming a
    /// possibly-poisoned chain. Returns true on the first transition into
    /// degraded so the caller can surface the banner exactly once.
    ///
    /// Falls back to a hard degrade only if no runtime exists to preserve.
    fn mark_runtime_degraded_preserving_context(&mut self) -> bool {
        let Some(runtime) = self.runtime.as_mut() else {
            return self.mark_runtime_degraded();
        };
        // restore_messages re-seeds the same history and clears the provider
        // thread id (chain), giving us "keep messages, reset chain" in one step.
        let preserved = runtime.session.messages().to_vec();
        runtime.session.restore_messages(preserved);
        if self.runtime_degraded {
            false
        } else {
            self.runtime_degraded = true;
            true
        }
    }

    fn rotate_for_new_thread_with<Init, Persist>(
        &mut self,
        runtime_generation: u64,
        initialize_runtime: Init,
        persist_runtime: Persist,
    ) -> Result<bool>
    where
        Init: FnOnce() -> Result<AgentRuntime>,
        Persist: FnOnce(&AgentRuntime) -> Result<()>,
    {
        let previous_runtime = self
            .runtime
            .as_ref()
            .filter(|runtime| !runtime.session.messages().is_empty());
        let should_persist_previous = previous_runtime.is_some();
        if let Some(runtime) = previous_runtime {
            persist_runtime(runtime)?;
        }

        self.runtime_generation = runtime_generation;
        match initialize_runtime() {
            Ok(runtime) => {
                self.runtime = Some(runtime);
                self.runtime_degraded = false;
                Ok(should_persist_previous)
            }
            Err(error) => {
                self.runtime = None;
                self.runtime_degraded = true;
                Err(error).context("Failed to initialize Agent runtime for new thread")
            }
        }
    }
}

fn current_agent_thread_generation() -> u64 {
    AGENT_THREAD_GENERATION.load(Ordering::SeqCst)
}

fn shared_agent_runtime_state_slot() -> &'static StdMutex<Option<Arc<TokioMutex<AgentRuntimeState>>>>
{
    SHARED_AGENT_RUNTIME_STATE.get_or_init(|| StdMutex::new(None))
}

fn shared_agent_runtime_state() -> Arc<TokioMutex<AgentRuntimeState>> {
    let mut guard = shared_agent_runtime_state_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_ref() {
        return Arc::clone(state);
    }

    let runtime_state = Arc::new(TokioMutex::new(AgentRuntimeState {
        runtime_generation: current_agent_thread_generation(),
        ..AgentRuntimeState::default()
    }));
    *guard = Some(Arc::clone(&runtime_state));
    runtime_state
}

pub(crate) fn request_new_agent_thread_boundary() -> u64 {
    let generation = AGENT_THREAD_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    debug!("Agent runtime thread boundary rotated (generation={generation})");
    generation
}

pub(crate) async fn reset_agent_runtime_for_new_thread() -> Result<u64> {
    let generation = request_new_agent_thread_boundary();
    let runtime_state = shared_agent_runtime_state();
    let mut guard = runtime_state.lock().await;

    match guard.rotate_for_new_thread_with(
        generation,
        initialize_agent_runtime,
        persist_runtime_thread,
    ) {
        Ok(_persisted_previous) => Ok(generation),
        Err(error) => Err(error),
    }
}

fn initialize_agent_runtime() -> Result<AgentRuntime> {
    let mut registry = ToolRegistry::new();
    crate::agent::tools::register_all_tools(&mut registry);

    let provider = crate::agent::create_default_provider()
        .context("Failed to create default agent provider")?;
    let (ui_tx, ui_rx) = mpsc::channel(AGENT_UI_CHANNEL_CAPACITY);
    let session = AgentSession::new(provider, Arc::new(registry), ui_tx);

    Ok(AgentRuntime {
        session,
        ui_rx,
        thread_store_id: ThreadStore::generate_id(),
    })
}

fn build_agent_stream_options(ai_assistive_max_tokens: i32) -> StreamOptions {
    let max_tokens = u32::try_from(ai_assistive_max_tokens)
        .ok()
        .filter(|tokens| *tokens > 0);

    // Model name comes from settings.json -> loader.rs -> env var. Keep a
    // release-safe OpenAI default so the agent path never falls into an empty
    // or provider-specific placeholder model.
    let model = std::env::var("LLM_ASSISTIVE_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(default_assistive_model);

    StreamOptions {
        model,
        system_prompt: Some(compose_agent_system_prompt()),
        max_tokens,
        temperature: None,
        // First-attempt default: preserve conversational chain. Session retry
        // path will clone+override this to true for retry attempts only.
        reset_chain: false,
    }
}

/// Compose the agent system prompt: the base assistive prompt plus a workspace
/// section that pins the configured project roots and tells the model to resolve
/// project names via `list_projects` instead of guessing filesystem paths.
fn compose_agent_system_prompt() -> String {
    let base = crate::config::get_assistive_prompt();
    let workspace = crate::agent::tools::workspace::workspace_prompt_section();
    format!("{base}\n\n{workspace}")
}

/// Title-case a `snake_case` / `kebab-case` identifier into readable words.
/// `brave_web_search` -> `Brave Web Search`.
fn prettify_identifier(s: &str) -> String {
    let cleaned = s.replace(['_', '-'], " ");
    let mut out = String::with_capacity(cleaned.len());
    for (i, word) in cleaned.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    if out.is_empty() { s.to_string() } else { out }
}

/// Map a raw tool identifier (often `mcp__<server>__<tool>`) to a concise,
/// human-readable label for the conversation timeline.
///
/// Collapsible Tool Evidence: raw MCP wire names like
/// `mcp__brave-search__brave_web_search` are transport noise in a conversation —
/// the user wants to read "Web search", not the addressing scheme. This is a pure
/// function so the mapping is unit-testable without a running UI.
pub(crate) fn friendly_tool_name(raw: &str) -> String {
    match raw {
        "mcp__brave-search__brave_web_search" | "brave_web_search" => return "Web search".into(),
        "mcp__brave-search__brave_local_search" | "brave_local_search" => {
            return "Local search".into();
        }
        "mcp__brave-search__brave_news_search" | "brave_news_search" => {
            return "News search".into();
        }
        "mcp__brave-search__brave_image_search" | "brave_image_search" => {
            return "Image search".into();
        }
        "mcp__brave-search__brave_video_search" | "brave_video_search" => {
            return "Video search".into();
        }
        "mcp__brave-search__brave_summarizer" | "brave_summarizer" => return "Summarize".into(),
        // Structural / intent / fleet MCP surfaces the operator named explicitly:
        // the generic `mcp__` fallback would read "Context · Loctree mcp", which is
        // both reversed and noisy. Pin the exact human labels here.
        "mcp__loctree-mcp__context" => return "Loctree context".into(),
        "mcp__loctree-mcp__find" => return "Loctree occurrences/find".into(),
        "mcp__aicx-mcp__aicx_intents" => return "AICX intents".into(),
        "mcp__vibecrafted-mcp__vc_run_observe" => return "Vibecrafted observe".into(),
        // Native (non-mcp) tools: the bare snake_case prettifies to a reversed,
        // verbose label ("Read Clipboard"); the operator wants noun-first copy.
        "read_clipboard" => return "Clipboard read".into(),
        "write_clipboard" => return "Clipboard write".into(),
        "take_screenshot" => return "Screenshot".into(),
        "transcribe_audio" => return "Audio transcription".into(),
        _ => {}
    }
    if let Some(rest) = raw.strip_prefix("mcp__") {
        let mut parts = rest.splitn(2, "__");
        let server = parts.next().unwrap_or("");
        let tool = parts.next().unwrap_or(server);
        // Trailing `__` with no tool segment (e.g. `mcp__github__`) yields an
        // empty `tool`. Without this guard the formatter below emits a dangling
        // " · Github" — the separator with nothing in front of it. Fall back to
        // the bare server label; if even the server is empty (`mcp__`), prettify
        // the raw rather than returning an empty string.
        if tool.is_empty() {
            return if server.is_empty() {
                prettify_identifier(raw)
            } else {
                prettify_identifier(server)
            };
        }
        let tool_pretty = prettify_identifier(tool);
        if server.is_empty() || tool == server {
            return tool_pretty;
        }
        return format!("{tool_pretty} · {}", prettify_identifier(server));
    }
    prettify_identifier(raw)
}

/// Drain a single agent UI event.
///
/// Legacy AppKit overlay delivery has been removed. This is still invoked from
/// the `run_agent_send_path` drain loop because consuming `ui_rx` events is what
/// advances `AgentSession::send` to completion (the channel is bounded). Only the
/// per-event overlay mutations are gone; debug logging of tool activity stays.
async fn apply_agent_ui_event(event: AgentUiEvent) {
    match event {
        AgentUiEvent::TextDelta(_)
        | AgentUiEvent::TextDone(_)
        | AgentUiEvent::ReasoningDelta(_)
        | AgentUiEvent::Done => {}
        AgentUiEvent::ToolExecuting { name, .. } => {
            debug!("Tool executing: {name} -> {}", friendly_tool_name(&name));
        }
        AgentUiEvent::ToolResult {
            name,
            summary,
            is_error,
            ..
        } => {
            debug!(
                "Tool result: {name} -> {} | is_error={is_error} | raw summary: {summary}",
                friendly_tool_name(&name)
            );
        }
        AgentUiEvent::Error(message) => {
            warn!("Agent runtime UI error event: {message}");
        }
    }
}

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

fn normalize_assistive_thread_text(text: &str) -> Option<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn persist_runtime_thread(runtime: &AgentRuntime) -> Result<()> {
    let store = ThreadStore::new().context("Failed to initialize ThreadStore")?;
    let now = Utc::now();
    let model = std::env::var("LLM_ASSISTIVE_MODEL").unwrap_or_else(|_| "unknown".to_string());

    let mut thread = store
        .load_thread(&runtime.thread_store_id)
        .unwrap_or_else(|_| Thread {
            id: runtime.thread_store_id.clone(),
            created_at: now,
            updated_at: now,
            title: "Codescribe Agent Chat".to_string(),
            title_is_custom: false,
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
    if !thread.title_is_custom {
        thread.title = derive_thread_title(runtime.session.messages());
    }
    thread.summary = derive_thread_summary(runtime.session.messages());
    thread.messages = runtime
        .session
        .messages()
        .iter()
        .map(ThreadMessage::from)
        .collect();
    thread.provider = "openai-responses".to_string();
    thread.model = model;

    store
        .save_thread(&thread)
        .context("Failed to persist agent thread to ThreadStore")?;
    Ok(())
}

fn persist_legacy_assistive_thread(user_text: &str, assistant_text: &str) -> Result<()> {
    let Some(user_text) = normalize_assistive_thread_text(user_text) else {
        return Ok(());
    };
    let Some(assistant_text) = normalize_assistive_thread_text(assistant_text) else {
        return Ok(());
    };

    let store = ThreadStore::new().context("Failed to initialize ThreadStore")?;
    let now = Utc::now();
    let model = std::env::var("LLM_ASSISTIVE_MODEL").unwrap_or_else(|_| "unknown".to_string());

    let mut title = user_text.chars().take(72).collect::<String>();
    if title.is_empty() {
        title = "Codescribe Agent Chat".to_string();
    }
    let mut summary = assistant_text.chars().take(240).collect::<String>();
    if summary.is_empty() {
        summary = "Assistant response".to_string();
    }
    let metadata = Some(json!({"source":"legacy-fallback"}));

    let thread = Thread {
        id: ThreadStore::generate_id(),
        created_at: now,
        updated_at: now,
        title,
        title_is_custom: false,
        mode: "assistive".to_string(),
        tags: vec![
            "agent".to_string(),
            "overlay".to_string(),
            "fallback".to_string(),
        ],
        notes: Vec::new(),
        messages: vec![
            ThreadMessage {
                role: "user".to_string(),
                content: vec![json!({"type":"input_text","text":user_text})],
                timestamp: now,
                metadata: metadata.clone(),
            },
            ThreadMessage {
                role: "assistant".to_string(),
                content: vec![json!({"type":"output_text","text":assistant_text})],
                timestamp: now,
                metadata,
            },
        ],
        summary: Some(summary),
        total_tokens: None,
        provider: "legacy-formatter".to_string(),
        model,
    };

    store
        .save_thread(&thread)
        .context("Failed to persist legacy assistive thread to ThreadStore")?;
    Ok(())
}

fn agent_send_error_allows_legacy_fallback(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    !message.starts_with("Provider stream error:")
}

/// P1.7: classify a send-path failure as transient (the provider blipped but
/// the conversation is still valid) vs hard (provider down / runtime cannot be
/// trusted). Transient failures get a SOFT degrade that preserves
/// `session.messages` and only resets the chain; hard failures drop the runtime.
///
/// This mirrors the core-side `is_transient_stream_start_error` heuristic; it is
/// duplicated app-side intentionally to avoid widening the core public surface
/// just for the controller's degrade policy.
fn agent_send_error_is_transient(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_lowercase();
    [
        "timeout",
        "timed out",
        "connection reset",
        "connection refused",
        "temporarily unavailable",
        "temporary failure",
        "broken pipe",
        "eof",
        "transport",
        "rate limit",
        "429",
        "502",
        "503",
    ]
    .iter()
    .any(|pattern| message.contains(pattern))
}

/// Maximum number of image attachments forwarded to the model per message.
/// Kept in sync with the legacy (`ai_formatting`) cap so both send paths behave
/// alike. Sized for real multi-image use (e.g. comparing several wireframes);
/// vision-capable backends accept far more, images are size-capped individually.
const MAX_AGENT_VISION_IMAGES: usize = 16;

/// Split an outgoing payload into its visible text and the loaded image
/// attachments referenced by the `ATTACHMENTS (image paths)` marker.
///
/// This is the fix for the attachment pipeline: the voice-chat send path appends
/// image paths to the payload as *text* (`build_attachments_block`). Without this
/// step the agent path forwarded them as plain text and the model never received
/// real vision input. Here we strip the marker block from the text and load each
/// image as bytes so `AgentSession::send` can emit proper `input_image` blocks.
///
/// Returns `(cleaned_text, loaded_images, dropped_names)`. `dropped_names` lists
/// images that could not be forwarded (missing/unreadable/too large) so the
/// caller can surface a visible attachment error instead of silently continuing.
fn build_image_attachments_from_text(text: &str) -> (String, Vec<ImageAttachment>, Vec<String>) {
    let (cleaned, mut paths) = codescribe_core::attachment::parse_image_attachment_block(text);

    if paths.is_empty() {
        return (cleaned, Vec::new(), Vec::new());
    }

    let mut dropped: Vec<String> = Vec::new();

    if paths.len() > MAX_AGENT_VISION_IMAGES {
        for extra in &paths[MAX_AGENT_VISION_IMAGES..] {
            dropped.push(file_label(extra));
        }
        warn!(
            "Too many image attachments ({}); forwarding first {} as vision input",
            paths.len(),
            MAX_AGENT_VISION_IMAGES
        );
        paths.truncate(MAX_AGENT_VISION_IMAGES);
    }

    let mut attachments = Vec::with_capacity(paths.len());
    for path in &paths {
        match codescribe_core::attachment::load_image_for_vision(
            path,
            codescribe_core::attachment::MAX_VISION_IMAGE_BYTES,
        ) {
            Some((data, media_type)) => attachments.push(ImageAttachment { data, media_type }),
            None => {
                warn!(
                    "Dropping image attachment (unsupported, unreadable, or too large): {}",
                    path.display()
                );
                dropped.push(file_label(path));
            }
        }
    }

    (cleaned, attachments, dropped)
}

/// Short, user-facing label for an attachment path (file name, path fallback).
fn file_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

async fn run_agent_send_path(
    runtime_state: &mut AgentRuntimeState,
    runtime_generation: u64,
    text: String,
    stream_options: StreamOptions,
) -> Result<()> {
    let (runtime, recovered_from_degraded) = match runtime_state.ensure_runtime(runtime_generation)
    {
        Ok(state) => state,
        Err(error) => {
            runtime_state.mark_runtime_degraded();
            return Err(error).context("Agent runtime unavailable");
        }
    };
    let _ = recovered_from_degraded;

    let send_result = {
        let (session, ui_rx) = (&mut runtime.session, &mut runtime.ui_rx);
        let (user_text, image_attachments, dropped_images) =
            build_image_attachments_from_text(&text);
        if !image_attachments.is_empty() {
            info!(
                "Agent send: forwarding {} image(s) as vision input",
                image_attachments.len()
            );
        }
        if !dropped_images.is_empty() {
            warn!(
                "Could not attach {} image(s) as vision input: {}",
                dropped_images.len(),
                dropped_images.join(", ")
            );
        }
        let send_future = session.send(user_text, image_attachments, &stream_options);
        tokio::pin!(send_future);

        let result = loop {
            tokio::select! {
                result = &mut send_future => break result,
                maybe_event = ui_rx.recv() => {
                    match maybe_event {
                        Some(event) => apply_agent_ui_event(event).await,
                        None => break Err(anyhow::anyhow!("Agent UI event channel closed")),
                    }
                }
            }
        };

        while let Ok(event) = ui_rx.try_recv() {
            apply_agent_ui_event(event).await;
        }

        result
    };

    match send_result {
        Ok(()) => {
            if let Err(error) = persist_runtime_thread(runtime) {
                warn!("Failed to persist agent thread: {}", error);
            }
            Ok(())
        }
        Err(error) => {
            if !agent_send_error_allows_legacy_fallback(&error) {
                return Ok(());
            }
            // P1.7: distinguish a transient provider blip (conversation still
            // valid -> keep messages, reset chain) from a hard failure (drop the
            // runtime). Both still mark the UI degraded and fall back to legacy.
            if agent_send_error_is_transient(&error) {
                runtime_state.mark_runtime_degraded_preserving_context();
            } else {
                runtime_state.mark_runtime_degraded();
            }
            Err(error).context("AgentSession send failed")
        }
    }
}

async fn run_legacy_send_path(
    text: &str,
    whisper_language: crate::config::Language,
) -> Option<String> {
    let result = crate::ai_formatting::format_text_with_status_channels(
        text,
        whisper_language.whisper_hint(),
        true,
        None,
        None,
    )
    .await;

    match result.status {
        crate::ai_formatting::AiFormatStatus::Applied
        | crate::ai_formatting::AiFormatStatus::AiNoop => Some(result.text),
        crate::ai_formatting::AiFormatStatus::Failed => Some("AI Failed".to_string()),
        crate::ai_formatting::AiFormatStatus::Skipped => None,
    }
}

async fn run_agent_send_with_fallback(
    runtime_state: &Arc<TokioMutex<AgentRuntimeState>>,
    text: String,
    whisper_language: crate::config::Language,
    ai_assistive_max_tokens: i32,
) {
    let _send_guard = AgentSendInFlightGuard::new();
    let stream_options = build_agent_stream_options(ai_assistive_max_tokens);
    let agent_result = {
        let mut guard = runtime_state.lock().await;
        let runtime_generation = current_agent_thread_generation();
        run_agent_send_path(&mut guard, runtime_generation, text.clone(), stream_options).await
    };

    if let Err(error) = agent_result {
        warn!("Agent fallback triggered: reason={}", error);
        warn!(
            "Agent runtime failed, switching this response to legacy fallback: {}",
            error
        );
        debug!("Legacy fallback input length: {}", text.len());
        let fallback_assistant_text = run_legacy_send_path(&text, whisper_language).await;
        if let Some(assistant_text) = fallback_assistant_text
            && let Err(error) = persist_legacy_assistive_thread(&text, &assistant_text)
        {
            warn!("Failed to persist legacy assistive fallback thread: {error}");
        }
    }
}

pub(crate) async fn send_assistive_with_agent_runtime(
    text: String,
    whisper_language: crate::config::Language,
    ai_assistive_max_tokens: i32,
) {
    let runtime_state = shared_agent_runtime_state();
    run_agent_send_with_fallback(
        &runtime_state,
        text,
        whisper_language,
        ai_assistive_max_tokens,
    )
    .await;
}

/// Legacy transcript backup for assistive mode is opt-in.
///
/// Non-assistive dictation keeps legacy transcript persistence unchanged.
pub fn raw_save_enabled(is_assistive: bool) -> bool {
    if !is_assistive {
        return true;
    }

    std::env::var(CODESCRIBE_ASSISTIVE_LEGACY_BACKUP_ENV)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

// ═══════════════════════════════════════════════════════════
// Event-based routing (new pipeline)
// ═══════════════════════════════════════════════════════════

use chrono::SecondsFormat;
use codescribe_core::ipc::{EngineEventWire, IpcEvent, IpcEventPayload};
use codescribe_core::pipeline::contracts::{EngineEvent, EventSink};
use tokio::sync::broadcast;

/// Session-level engine stats snapshot used by controller decisions.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionEngineStats {
    pub hallucination_drops: u64,
    pub semantic_gate_drops: u64,
    pub filtered_empty_drops: u64,
    pub corrections_applied: u64,
    pub total_utterances: u64,
    pub dropped_audio_chunks: u64,
    pub partial_runs_total: u64,
    pub trigger_utterance_count: u64,
    pub trigger_speech_count: u64,
    pub trigger_timer_count: u64,
    pub partial_stale_count: u64,
    pub partial_coalesced_count: u64,
    pub partial_dropped_count: u64,
}

/// Session telemetry captured from `EngineEvent`s.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionTelemetrySnapshot {
    pub no_speech_reason: Option<String>,
    pub stats: Option<SessionEngineStats>,
}

pub(crate) type SharedSessionTelemetry = Arc<StdMutex<SessionTelemetrySnapshot>>;

pub(crate) fn new_session_telemetry() -> SharedSessionTelemetry {
    Arc::new(StdMutex::new(SessionTelemetrySnapshot::default()))
}

pub(crate) fn reset_session_telemetry(shared: &SharedSessionTelemetry) {
    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    *guard = SessionTelemetrySnapshot::default();
}

pub(crate) fn snapshot_session_telemetry(
    shared: &SharedSessionTelemetry,
) -> SessionTelemetrySnapshot {
    shared.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Captures `NoSpeech`/`Stats` telemetry for controller-level routing decisions.
pub(crate) struct SessionTelemetrySink {
    shared: SharedSessionTelemetry,
}

impl SessionTelemetrySink {
    pub(crate) fn new(shared: SharedSessionTelemetry) -> Self {
        Self { shared }
    }
}

/// Broadcasts sanitized engine events to IPC subscribers.
pub(crate) struct IpcBroadcastSink {
    tx: broadcast::Sender<IpcEvent>,
}

impl IpcBroadcastSink {
    pub(crate) fn new(tx: broadcast::Sender<IpcEvent>) -> Self {
        Self { tx }
    }
}

impl EventSink for IpcBroadcastSink {
    fn on_event(&self, event: &EngineEvent) {
        let ipc_event = IpcEvent {
            timestamp: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            payload: IpcEventPayload::Engine(EngineEventWire::from(event)),
        };
        let _ = self.tx.send(ipc_event);
    }
}

impl EventSink for SessionTelemetrySink {
    fn on_event(&self, event: &EngineEvent) {
        let mut guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            EngineEvent::NoSpeech { reason } => {
                guard.no_speech_reason = Some(reason.clone());
            }
            EngineEvent::Stats {
                hallucination_drops,
                semantic_gate_drops,
                filtered_empty_drops,
                corrections_applied,
                total_utterances,
                dropped_audio_chunks,
                partial_runs_total,
                trigger_utterance_count,
                trigger_speech_count,
                trigger_timer_count,
                partial_stale_count,
                partial_coalesced_count,
                partial_dropped_count,
            } => {
                guard.stats = Some(SessionEngineStats {
                    hallucination_drops: *hallucination_drops,
                    semantic_gate_drops: *semantic_gate_drops,
                    filtered_empty_drops: *filtered_empty_drops,
                    corrections_applied: *corrections_applied,
                    total_utterances: *total_utterances,
                    dropped_audio_chunks: *dropped_audio_chunks,
                    partial_runs_total: *partial_runs_total,
                    trigger_utterance_count: *trigger_utterance_count,
                    trigger_speech_count: *trigger_speech_count,
                    trigger_timer_count: *trigger_timer_count,
                    partial_stale_count: *partial_stale_count,
                    partial_coalesced_count: *partial_coalesced_count,
                    partial_dropped_count: *partial_dropped_count,
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use codescribe_core::agent::{AgentEvent, AgentProvider, ToolDefinition};
    use std::sync::atomic::AtomicUsize;

    // ── Collapsible Tool Evidence: friendly tool-name mapping ───────────────

    #[test]
    fn friendly_tool_name_maps_known_brave_tools() {
        assert_eq!(
            friendly_tool_name("mcp__brave-search__brave_web_search"),
            "Web search"
        );
        assert_eq!(friendly_tool_name("brave_web_search"), "Web search");
        assert_eq!(
            friendly_tool_name("mcp__brave-search__brave_news_search"),
            "News search"
        );
    }

    #[test]
    fn friendly_tool_name_prettifies_unknown_mcp_tools() {
        // Unknown mcp__server__tool falls back to "<Tool> · <Server>" — never the
        // raw wire name in the conversation timeline.
        assert_eq!(
            friendly_tool_name("mcp__github__create_issue"),
            "Create Issue · Github"
        );
        // Bare snake_case identifier is title-cased.
        assert_eq!(friendly_tool_name("read_file"), "Read File");
        // The raw mcp__ wire form must never survive verbatim.
        assert!(!friendly_tool_name("mcp__github__create_issue").contains("mcp__"));
        // Trailing `__` leaves an empty tool segment (`mcp__github__`). This must
        // collapse to the bare server label — never a dangling " · Github" with
        // the separator floating in front of nothing.
        assert_eq!(friendly_tool_name("mcp__github__"), "Github");
        assert!(!friendly_tool_name("mcp__github__").contains('·'));
        assert!(!friendly_tool_name("mcp__github__").starts_with(' '));
        // Fully degenerate `mcp__` (no server, no tool) must not yield an empty
        // label either.
        assert!(!friendly_tool_name("mcp__").is_empty());
    }

    #[test]
    fn friendly_tool_name_honors_operator_label_table() {
        // The operator's explicit raw→label table. Before this mapping these all
        // fell into the generic `mcp__` / prettify fallback and read reversed or
        // noisy (e.g. "Context · Loctree mcp", "Read Clipboard").
        assert_eq!(
            friendly_tool_name("mcp__loctree-mcp__context"),
            "Loctree context"
        );
        assert_eq!(
            friendly_tool_name("mcp__loctree-mcp__find"),
            "Loctree occurrences/find"
        );
        assert_eq!(
            friendly_tool_name("mcp__aicx-mcp__aicx_intents"),
            "AICX intents"
        );
        assert_eq!(
            friendly_tool_name("mcp__vibecrafted-mcp__vc_run_observe"),
            "Vibecrafted observe"
        );
        assert_eq!(friendly_tool_name("read_clipboard"), "Clipboard read");
        assert_eq!(friendly_tool_name("write_clipboard"), "Clipboard write");
        assert_eq!(friendly_tool_name("take_screenshot"), "Screenshot");
        assert_eq!(
            friendly_tool_name("transcribe_audio"),
            "Audio transcription"
        );
    }

    #[test]
    fn regression_sequence_raw_names_produce_expected_runtime_labels() {
        // Operator regression scenario: the grouped block must show exactly these
        // labels at runtime — not just in the pure-module test that hardcodes the
        // display_name. This proves the controller maps the raw wire names the same
        // way the timeline expects.
        assert_eq!(
            friendly_tool_name("mcp__brave-search__brave_web_search"),
            "Web search"
        );
        assert_eq!(
            friendly_tool_name("mcp__loctree-mcp__context"),
            "Loctree context"
        );
        assert_eq!(
            friendly_tool_name("mcp__aicx-mcp__aicx_intents"),
            "AICX intents"
        );
    }

    struct NoopTestProvider;

    #[async_trait]
    impl AgentProvider for NoopTestProvider {
        async fn stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &StreamOptions,
        ) -> anyhow::Result<mpsc::Receiver<AgentEvent>> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }

        fn build_tool_result(
            &self,
            call_id: &str,
            content: Vec<ContentBlock>,
            is_error: bool,
        ) -> Message {
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: call_id.to_string(),
                    content,
                    is_error,
                }],
            )
        }

        fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
            ContentBlock::Image {
                data: data.to_vec(),
                media_type: media_type.to_string(),
            }
        }

        fn name(&self) -> &str {
            "noop-test-provider"
        }
    }

    fn runtime_with_thread_id(thread_store_id: &str) -> AgentRuntime {
        let (ui_tx, ui_rx) = mpsc::channel(8);
        let session = AgentSession::new(
            Box::new(NoopTestProvider),
            Arc::new(ToolRegistry::new()),
            ui_tx,
        );
        AgentRuntime {
            session,
            ui_rx,
            thread_store_id: thread_store_id.to_string(),
        }
    }

    fn seed_runtime_with_user_message(runtime: &mut AgentRuntime) {
        let options = StreamOptions {
            model: String::new(),
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            reset_chain: false,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should initialize");
        rt.block_on(
            runtime
                .session
                .send("hello".to_string(), Vec::new(), &options),
        )
        .expect("seed message should be recorded");
    }

    #[test]
    fn test_session_telemetry_sink_tracks_no_speech_and_stats() {
        let shared = new_session_telemetry();
        let sink = SessionTelemetrySink::new(Arc::clone(&shared));

        sink.on_event(&EngineEvent::NoSpeech {
            reason: "vad_no_speech_detected".to_string(),
        });
        sink.on_event(&EngineEvent::Stats {
            dropped_audio_chunks: 3,
            hallucination_drops: 2,
            semantic_gate_drops: 1,
            filtered_empty_drops: 4,
            corrections_applied: 5,
            total_utterances: 0,
            partial_runs_total: 6,
            trigger_utterance_count: 2,
            trigger_speech_count: 3,
            trigger_timer_count: 1,
            partial_stale_count: 7,
            partial_coalesced_count: 8,
            partial_dropped_count: 9,
        });

        let snapshot = snapshot_session_telemetry(&shared);
        assert_eq!(
            snapshot.no_speech_reason.as_deref(),
            Some("vad_no_speech_detected")
        );
        let stats = snapshot.stats.expect("stats should be captured");
        assert_eq!(stats.hallucination_drops, 2);
        assert_eq!(stats.semantic_gate_drops, 1);
        assert_eq!(stats.filtered_empty_drops, 4);
        assert_eq!(stats.corrections_applied, 5);
        assert_eq!(stats.total_utterances, 0);
        assert_eq!(stats.dropped_audio_chunks, 3);
        assert_eq!(stats.partial_runs_total, 6);
        assert_eq!(stats.trigger_utterance_count, 2);
        assert_eq!(stats.trigger_speech_count, 3);
        assert_eq!(stats.trigger_timer_count, 1);
        assert_eq!(stats.partial_stale_count, 7);
        assert_eq!(stats.partial_coalesced_count, 8);
        assert_eq!(stats.partial_dropped_count, 9);
    }

    #[test]
    fn test_reset_session_telemetry_clears_snapshot() {
        let shared = new_session_telemetry();
        {
            let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
            guard.no_speech_reason = Some("test".to_string());
            guard.stats = Some(SessionEngineStats {
                hallucination_drops: 1,
                ..Default::default()
            });
        }
        reset_session_telemetry(&shared);

        let snapshot = snapshot_session_telemetry(&shared);
        assert!(snapshot.no_speech_reason.is_none());
        assert!(snapshot.stats.is_none());
    }

    #[test]
    fn test_request_new_agent_thread_boundary_is_monotonic() {
        let before = current_agent_thread_generation();
        let next = request_new_agent_thread_boundary();
        let now = current_agent_thread_generation();

        assert!(next > before);
        assert!(now >= next);
    }

    #[test]
    fn test_runtime_generation_reuses_existing_runtime_when_unchanged() {
        let mut runtime_state = AgentRuntimeState {
            runtime: Some(runtime_with_thread_id("thread_existing")),
            runtime_generation: 41,
            runtime_degraded: false,
        };
        let init_calls = AtomicUsize::new(0);

        let (runtime, recovered) = runtime_state
            .ensure_runtime_with(41, || {
                init_calls.fetch_add(1, Ordering::SeqCst);
                Ok(runtime_with_thread_id("thread_should_not_be_used"))
            })
            .expect("runtime should be reused for unchanged generation");

        assert_eq!(runtime.thread_store_id, "thread_existing");
        assert_eq!(init_calls.load(Ordering::SeqCst), 0);
        assert!(!recovered);
        assert_eq!(runtime_state.runtime_generation, 41);
    }

    #[test]
    fn test_runtime_generation_change_rotates_runtime_identity() {
        let mut runtime_state = AgentRuntimeState {
            runtime: Some(runtime_with_thread_id("thread_old")),
            runtime_generation: 12,
            runtime_degraded: false,
        };
        let init_calls = AtomicUsize::new(0);

        let (runtime, recovered) = runtime_state
            .ensure_runtime_with(13, || {
                init_calls.fetch_add(1, Ordering::SeqCst);
                Ok(runtime_with_thread_id("thread_new"))
            })
            .expect("runtime should rotate after generation change");

        assert_eq!(init_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.thread_store_id, "thread_new");
        assert_eq!(runtime_state.runtime_generation, 13);
        assert!(!recovered);
    }

    #[test]
    fn test_new_thread_boundary_forces_fresh_runtime_identity() {
        let mut runtime_state = AgentRuntimeState {
            runtime: Some(runtime_with_thread_id("thread_before_boundary")),
            runtime_generation: current_agent_thread_generation(),
            runtime_degraded: false,
        };
        let new_generation = request_new_agent_thread_boundary();
        let init_calls = AtomicUsize::new(0);

        let (runtime, recovered) = runtime_state
            .ensure_runtime_with(new_generation, || {
                init_calls.fetch_add(1, Ordering::SeqCst);
                Ok(runtime_with_thread_id("thread_after_boundary"))
            })
            .expect("runtime should rotate after explicit boundary request");

        assert_eq!(init_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.thread_store_id, "thread_after_boundary");
        assert_eq!(runtime_state.runtime_generation, new_generation);
        assert!(!recovered);
    }

    #[test]
    fn test_runtime_recovery_clears_degraded_flag_on_reinit() {
        let mut runtime_state = AgentRuntimeState {
            runtime: None,
            runtime_generation: 7,
            runtime_degraded: true,
        };
        let init_calls = AtomicUsize::new(0);

        let (runtime, recovered) = runtime_state
            .ensure_runtime_with(7, || {
                init_calls.fetch_add(1, Ordering::SeqCst);
                Ok(runtime_with_thread_id("thread_recovered"))
            })
            .expect("runtime should reinitialize after degraded state");

        assert_eq!(init_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.thread_store_id, "thread_recovered");
        assert!(recovered);
        assert!(!runtime_state.runtime_degraded);
    }

    #[test]
    fn test_provider_stream_errors_skip_legacy_fallback() {
        let error = anyhow::anyhow!(
            "Provider stream error: Agent SSE error internal_error: 'list' object has no attribute 'uid'"
        );

        assert!(!agent_send_error_allows_legacy_fallback(&error));
    }

    /// Provider that completes one clean turn so the seeded session ends up with
    /// both conversation history AND a provider thread id (chain) set.
    struct CompletingTestProvider;

    #[async_trait]
    impl AgentProvider for CompletingTestProvider {
        async fn stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &StreamOptions,
        ) -> anyhow::Result<mpsc::Receiver<AgentEvent>> {
            let (tx, rx) = mpsc::channel(4);
            tx.send(AgentEvent::TextDone("hi back".to_string()))
                .await
                .expect("test channel should accept text");
            tx.send(AgentEvent::ResponseDone {
                response_id: Some("resp_seed".to_string()),
                clean: true,
            })
            .await
            .expect("test channel should accept completion");
            Ok(rx)
        }

        fn build_tool_result(
            &self,
            call_id: &str,
            content: Vec<ContentBlock>,
            is_error: bool,
        ) -> Message {
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: call_id.to_string(),
                    content,
                    is_error,
                }],
            )
        }

        fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
            ContentBlock::Image {
                data: data.to_vec(),
                media_type: media_type.to_string(),
            }
        }

        fn name(&self) -> &str {
            "completing-test-provider"
        }
    }

    fn seed_completed_runtime(thread_store_id: &str) -> AgentRuntime {
        let (ui_tx, ui_rx) = mpsc::channel(8);
        let mut session = AgentSession::new(
            Box::new(CompletingTestProvider),
            Arc::new(ToolRegistry::new()),
            ui_tx,
        );
        let options = StreamOptions {
            model: String::new(),
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            reset_chain: false,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should initialize");
        rt.block_on(session.send("hello".to_string(), Vec::new(), &options))
            .expect("seed turn should complete");
        AgentRuntime {
            session,
            ui_rx,
            thread_store_id: thread_store_id.to_string(),
        }
    }

    /// P1.7: a transient in-conversation failure must SOFT-degrade — keep the
    /// runtime and its `session.messages`, and reset only the chain. The proof:
    /// messages survive (history non-empty) while the provider thread id (chain)
    /// is cleared so the next turn full-replays.
    #[test]
    fn degrade_preserves_messages_on_transient() {
        let runtime = seed_completed_runtime("thread_transient");
        assert!(
            !runtime.session.messages().is_empty(),
            "seed must produce conversation history"
        );
        assert_eq!(
            runtime.session.thread_id(),
            Some("resp_seed"),
            "seed must set the provider chain id"
        );

        let mut runtime_state = AgentRuntimeState {
            runtime: Some(runtime),
            runtime_generation: 3,
            runtime_degraded: false,
        };

        let transient = anyhow::anyhow!("Failed to start 'openai' streaming")
            .context("connection reset by peer");
        assert!(
            agent_send_error_is_transient(&transient),
            "connection-reset error must classify as transient"
        );

        let newly_degraded = runtime_state.mark_runtime_degraded_preserving_context();
        assert!(newly_degraded, "first soft degrade transitions the flag");

        let runtime = runtime_state
            .runtime
            .as_ref()
            .expect("soft degrade must keep the runtime alive");
        assert!(
            !runtime.session.messages().is_empty(),
            "transient degrade must preserve session.messages"
        );
        assert_eq!(
            runtime.session.thread_id(),
            None,
            "transient degrade must reset the chain so the next turn replays"
        );
        assert!(runtime_state.runtime_degraded);
    }

    /// Counterpart: a hard (non-transient) failure drops the runtime entirely.
    #[test]
    fn hard_degrade_drops_runtime_on_non_transient() {
        let mut runtime_state = AgentRuntimeState {
            runtime: Some(seed_completed_runtime("thread_hard")),
            runtime_generation: 5,
            runtime_degraded: false,
        };

        let hard = anyhow::anyhow!("Agent runtime was not initialized");
        assert!(
            !agent_send_error_is_transient(&hard),
            "init failure must NOT classify as transient"
        );

        runtime_state.mark_runtime_degraded();
        assert!(
            runtime_state.runtime.is_none(),
            "hard degrade must drop the runtime"
        );
        assert!(runtime_state.runtime_degraded);
    }

    #[test]
    fn test_agent_send_in_flight_guard_tracks_nested_sends() {
        set_agent_send_in_flight_for_test(false);
        assert!(!is_agent_send_in_flight());

        let first_guard = AgentSendInFlightGuard::new();
        assert!(is_agent_send_in_flight());

        {
            let second_guard = AgentSendInFlightGuard::new();
            assert!(is_agent_send_in_flight());
            drop(second_guard);
            assert!(is_agent_send_in_flight());
        }

        drop(first_guard);
        assert!(!is_agent_send_in_flight());
    }

    #[test]
    fn test_runtime_unavailable_errors_allow_legacy_fallback() {
        let error = anyhow::anyhow!("Agent runtime unavailable");

        assert!(agent_send_error_allows_legacy_fallback(&error));
    }

    #[test]
    fn test_rotate_for_new_thread_persists_previous_thread_with_messages() {
        let mut old_runtime = runtime_with_thread_id("thread_old");
        seed_runtime_with_user_message(&mut old_runtime);
        let mut runtime_state = AgentRuntimeState {
            runtime: Some(old_runtime),
            runtime_generation: 21,
            runtime_degraded: false,
        };
        let persist_calls = AtomicUsize::new(0);

        let persisted = runtime_state
            .rotate_for_new_thread_with(
                22,
                || Ok(runtime_with_thread_id("thread_new")),
                |runtime| {
                    persist_calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(runtime.thread_store_id, "thread_old");
                    assert_eq!(runtime.session.messages().len(), 1);
                    Ok(())
                },
            )
            .expect("runtime rotation should succeed");

        assert!(persisted);
        assert_eq!(persist_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime_state.runtime_generation, 22);
        assert!(!runtime_state.runtime_degraded);
        let runtime = runtime_state
            .runtime
            .expect("new runtime should be installed");
        assert_eq!(runtime.thread_store_id, "thread_new");
        assert!(runtime.session.messages().is_empty());
    }

    #[test]
    fn test_rotate_for_new_thread_skips_persist_when_empty() {
        let mut runtime_state = AgentRuntimeState {
            runtime: Some(runtime_with_thread_id("thread_old")),
            runtime_generation: 4,
            runtime_degraded: false,
        };
        let persist_calls = AtomicUsize::new(0);

        let persisted = runtime_state
            .rotate_for_new_thread_with(
                5,
                || Ok(runtime_with_thread_id("thread_new")),
                |_runtime| {
                    persist_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            )
            .expect("runtime rotation should succeed");

        assert!(!persisted);
        assert_eq!(persist_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_rotate_for_new_thread_marks_degraded_when_reinit_fails() {
        let mut runtime_state = AgentRuntimeState {
            runtime: Some(runtime_with_thread_id("thread_old")),
            runtime_generation: 11,
            runtime_degraded: false,
        };
        let result = runtime_state.rotate_for_new_thread_with(
            12,
            || Err(anyhow::anyhow!("boom")),
            |_runtime| Ok(()),
        );

        assert!(result.is_err());
        assert_eq!(runtime_state.runtime_generation, 12);
        assert!(runtime_state.runtime_degraded);
        assert!(runtime_state.runtime.is_none());
    }

    #[test]
    fn test_build_image_attachments_passthrough_without_marker() {
        let text = "plain message, no attachments";
        let (cleaned, images, dropped) = build_image_attachments_from_text(text);
        assert_eq!(cleaned, text);
        assert!(images.is_empty());
        assert!(dropped.is_empty());
    }

    #[test]
    fn test_build_image_attachments_loads_real_image_and_reports_dropped() {
        let dir = std::env::temp_dir().join(format!("cs_helpers_vision_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let img = dir.join("shot.png");
        std::fs::write(&img, b"\x89PNG\r\n\x1a\nfake").unwrap();
        let missing = dir.join("gone.png");

        let text = format!(
            "describe these\n\n---\nATTACHMENTS (image paths)\n- {}\n- {}\n",
            img.display(),
            missing.display()
        );
        let (cleaned, images, dropped) = build_image_attachments_from_text(&text);

        // Marker block and raw paths are gone from the model-visible text.
        assert!(!cleaned.contains("ATTACHMENTS (image paths)"));
        assert!(!cleaned.contains(&img.display().to_string()));
        assert!(cleaned.contains("describe these"));

        // Only the readable image becomes a real vision attachment; the missing
        // one is reported as dropped (visible error), never forwarded as text.
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].media_type, "image/png");
        assert!(!images[0].data.is_empty());
        assert_eq!(dropped, vec!["gone.png".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_image_attachments_caps_and_reports_overflow() {
        let dir = std::env::temp_dir().join(format!("cs_helpers_cap_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut lines = String::from("multi\n\nATTACHMENTS (image paths)\n");
        for i in 0..(MAX_AGENT_VISION_IMAGES + 2) {
            let p = dir.join(format!("img{i}.png"));
            std::fs::write(&p, b"\x89PNG\r\n\x1a\nfake").unwrap();
            lines.push_str(&format!("- {}\n", p.display()));
        }
        let (_cleaned, images, dropped) = build_image_attachments_from_text(&lines);

        // Cap honored, overflow surfaced (not silently dropped).
        assert_eq!(images.len(), MAX_AGENT_VISION_IMAGES);
        assert_eq!(dropped.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
