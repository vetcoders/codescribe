//! Controller helper functions
//!
//! Session state management and utility functions.

use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Mutex as TokioMutex, mpsc};
use tracing::{debug, info, warn};

use crate::agent_delivery::{AgentDeliveryEvent, register_agent_delivery_turn};
use anyhow::{Context, Result};
use codescribe_core::agent::{
    AgentSession, AgentUiEvent, ImageAttachment, Message, StreamOptions, ThreadDeliveryGateway,
    ThreadDeliveryInput, ThreadDeliveryReceipt, ThreadDeliverySource, ThreadMessage, ThreadStore,
    ToolRegistry,
};
use codescribe_core::config::Config;
use codescribe_core::llm::lane_truth;
use serde_json::json;

use crate::os::hold_badge::{BadgeMode, show_badge_for_mode};
use crate::os::tray_status;

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
    tray_status::set_tray_assistive_session(is_assistive);
}

/// Publish one canonical recording-indicator state to every Rust-owned sink.
/// Swift receives the same `BadgeMode` through the tray-status bridge, so the
/// cursor badge, menu glyph, and overlay spectrometer cannot drift by inventing
/// their own lane enums.
pub fn publish_recording_indicator(mode: BadgeMode, show_cursor_badge: bool) {
    IS_ASSISTIVE_SESSION.store(mode == BadgeMode::Assistive, Ordering::SeqCst);
    tray_status::set_tray_indicator_mode(mode);
    if show_cursor_badge {
        show_badge_for_mode(mode);
    }
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
/// - Every dictation session publishes the same engine events over IPC/FFI.
/// - `delta` must already follow `TranscriptDelta` backspace semantics.
///   This function must never receive full preview snapshots.
pub fn route_transcription_delta(_delta: &str) {
    // Legacy AppKit overlay delivery removed. Assistive deltas reach SwiftUI via
    // the engine event broadcast (see IpcBroadcastSink / subscribe_events).
}

/// DeltaSink that routes deltas to the active UI overlay.
///
/// Plugs into `PresentationEmitter` → `BufferedEmitter` → delta chain.
pub struct RoutingDeltaSink;

impl codescribe_core::pipeline::contracts::DeltaSink for RoutingDeltaSink {
    fn apply(&self, delta: &codescribe_core::pipeline::contracts::TranscriptDelta) {
        route_transcription_delta(&delta.delta);
    }
}

const AGENT_UI_CHANNEL_CAPACITY: usize = 256;
static AGENT_SEND_IN_FLIGHT_COUNT: AtomicUsize = AtomicUsize::new(0);
static SHARED_AGENT_RUNTIME_STATE: OnceLock<StdMutex<Option<Arc<TokioMutex<AgentRuntimeState>>>>> =
    OnceLock::new();

struct AgentRuntime {
    session: AgentSession,
    ui_rx: mpsc::Receiver<AgentUiEvent>,
    thread_store_id: String,
    /// Cancellation restores local history immediately; the next request must
    /// also clear any provider-owned response chain before replaying that history.
    reset_chain_on_next_send: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentSendOutcome {
    Completed,
    Cancelled,
}

#[derive(Default)]
struct AgentRuntimeState {
    runtime: Option<AgentRuntime>,
    /// Durable backend thread identity. Recorded when a runtime is installed and
    /// retained across `runtime = None`, so a rebuilt runtime rejoins the same
    /// thread (and its persisted history) instead of silently starting a new one.
    thread_store_id: Option<String>,
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
    fn ensure_runtime(&mut self) -> Result<(&mut AgentRuntime, bool)> {
        self.ensure_runtime_with(initialize_agent_runtime, rehydrate_thread_messages)
    }

    /// Install a runtime if none is live. Ordinary consecutive sends reuse the
    /// existing runtime untouched — identity and history never rotate here.
    ///
    /// A rebuild after `runtime = None` (hard degrade) rejoins the durable
    /// `thread_store_id` and rehydrates the last successfully persisted history
    /// through `load_persisted_history`, so the next provider call replays the
    /// prior conversation instead of silently starting a new thread. A failed
    /// rehydration keeps the stable identity and surfaces explicit recovery
    /// evidence; it never mints a fresh thread id.
    fn ensure_runtime_with<Init, Load>(
        &mut self,
        initialize_runtime: Init,
        load_persisted_history: Load,
    ) -> Result<(&mut AgentRuntime, bool)>
    where
        Init: FnOnce() -> Result<AgentRuntime>,
        Load: FnOnce(&str) -> Result<Option<Vec<Message>>>,
    {
        let mut recovered_from_degraded = false;
        if self.runtime.is_none() {
            let mut runtime = initialize_runtime()?;
            match self.thread_store_id.clone() {
                Some(thread_store_id) => {
                    runtime.thread_store_id = thread_store_id.clone();
                    match load_persisted_history(&thread_store_id) {
                        Ok(Some(messages)) if !messages.is_empty() => {
                            let rehydrated_message_count = messages.len();
                            // restore_messages also clears the provider chain, so
                            // the next send full-replays the restored history.
                            runtime.session.restore_messages(messages);
                            info!(
                                thread_store_id = %thread_store_id,
                                recovery_class = "rehydrated",
                                rehydrated_message_count,
                                "Agent runtime rebuilt onto durable thread with persisted history"
                            );
                        }
                        Ok(_) => {
                            info!(
                                thread_store_id = %thread_store_id,
                                recovery_class = "rehydrate_empty",
                                rehydrated_message_count = 0usize,
                                "Agent runtime rebuilt onto durable thread; no persisted history to restore"
                            );
                        }
                        Err(error) => {
                            warn!(
                                thread_store_id = %thread_store_id,
                                recovery_class = "rehydrate_failed",
                                error = %error,
                                "Agent runtime rebuilt onto durable thread but history rehydration failed; continuing with empty history on the same thread"
                            );
                        }
                    }
                }
                None => {
                    info!(
                        thread_store_id = %runtime.thread_store_id,
                        recovery_class = "fresh_thread",
                        "Agent runtime installed with new durable thread identity"
                    );
                    self.thread_store_id = Some(runtime.thread_store_id.clone());
                }
            }
            self.runtime = Some(runtime);
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
    /// failed). Drops the runtime — in-memory history is lost — but keeps the
    /// durable `thread_store_id`, so the next `ensure_runtime` rebuild rejoins
    /// the same backend thread and rehydrates its persisted history.
    fn mark_runtime_degraded(&mut self, reason: &'static str) -> bool {
        let dropped_message_count = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.session.messages().len())
            .unwrap_or(0);
        self.runtime = None;
        warn!(
            thread_store_id = self.thread_store_id.as_deref().unwrap_or("<unassigned>"),
            recovery_class = "hard_degrade",
            reason,
            dropped_message_count,
            "Agent runtime hard-degraded; durable thread identity retained for rehydration"
        );
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
    fn mark_runtime_degraded_preserving_context(&mut self, reason: &'static str) -> bool {
        let Some(runtime) = self.runtime.as_mut() else {
            return self.mark_runtime_degraded(reason);
        };
        // restore_messages re-seeds the same history and clears the provider
        // thread id (chain), giving us "keep messages, reset chain" in one step.
        let preserved = runtime.session.messages().to_vec();
        let preserved_message_count = preserved.len();
        let thread_store_id = runtime.thread_store_id.clone();
        runtime.session.restore_messages(preserved);
        warn!(
            thread_store_id = %thread_store_id,
            recovery_class = "soft_degrade",
            reason,
            preserved_message_count,
            "Agent runtime soft-degraded; history preserved, provider chain reset"
        );
        if self.runtime_degraded {
            false
        } else {
            self.runtime_degraded = true;
            true
        }
    }
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

    let runtime_state = Arc::new(TokioMutex::new(AgentRuntimeState::default()));
    *guard = Some(Arc::clone(&runtime_state));
    runtime_state
}

/// Load the persisted messages of a durable thread from the canonical
/// ThreadStore so a rebuilt runtime can rehydrate. `Ok(None)` means no artifact
/// exists yet (the thread degraded before its first successful persist).
fn rehydrate_thread_messages(thread_store_id: &str) -> Result<Option<Vec<Message>>> {
    let store = ThreadStore::new().context("Failed to open ThreadStore for rehydration")?;
    load_thread_messages_from(&store, thread_store_id)
}

fn load_thread_messages_from(
    store: &ThreadStore,
    thread_store_id: &str,
) -> Result<Option<Vec<Message>>> {
    if !store.thread_file_path(thread_store_id)?.exists() {
        return Ok(None);
    }
    let thread = store.load_thread(thread_store_id)?;
    Ok(Some(
        thread
            .messages
            .iter()
            .map(ThreadMessage::to_message)
            .collect(),
    ))
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
        reset_chain_on_next_send: false,
    })
}

fn build_agent_stream_options(
    ai_assistive_max_tokens: i32,
    use_assistive_persona: bool,
) -> StreamOptions {
    let max_tokens = u32::try_from(ai_assistive_max_tokens)
        .ok()
        .filter(|tokens| *tokens > 0);

    let (_, model) = lane_truth::assistive_identity(&Config::load());

    StreamOptions {
        model,
        system_prompt: Some(compose_agent_system_prompt(use_assistive_persona)),
        max_tokens,
        temperature: None,
        // First-attempt default: preserve conversational chain. Session retry
        // path will clone+override this to true for retry attempts only.
        reset_chain: false,
    }
}

/// Compose the agent system prompt.
///
/// - `use_assistive_persona=true` (act-on-selection lane): base is `assistive.txt`.
/// - `use_assistive_persona=false` (voice-chat lane, W10-D): agent persona only —
///   workspace + doctrine, no "text assistant" identity.
fn compose_agent_system_prompt(use_assistive_persona: bool) -> String {
    let workspace = crate::agent::tools::workspace::workspace_prompt_section();
    let doctrine = crate::agent::tools::doctrine::review_doctrine_prompt_section();
    if use_assistive_persona {
        let base = crate::config::get_assistive_prompt();
        format!("{base}\n\n{workspace}\n\n{doctrine}")
    } else {
        format!(
            "You are the Codescribe agent. Answer and act on the user's spoken request using the available tools when helpful.\n\n{workspace}\n\n{doctrine}"
        )
    }
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

/// Translate a core `AgentUiEvent` into the voice-assistive delivery event the
/// bridge forwards to the SwiftUI AgentChat. 1:1 field mapping — the two enums
/// deliberately share the same shape so the Swift listener is symmetric to the
/// composer's `CsAgentListener`.
fn agent_ui_event_to_delivery(event: &AgentUiEvent) -> AgentDeliveryEvent {
    match event {
        AgentUiEvent::TextDelta(delta) => AgentDeliveryEvent::TextDelta(delta.clone()),
        AgentUiEvent::TextDone(text) => AgentDeliveryEvent::TextDone(text.clone()),
        AgentUiEvent::ReasoningDelta(delta) => AgentDeliveryEvent::ReasoningDelta(delta.clone()),
        AgentUiEvent::ToolExecuting { name, id } => AgentDeliveryEvent::ToolExecuting {
            name: name.clone(),
            id: id.clone(),
        },
        AgentUiEvent::ToolResult {
            name,
            id,
            summary,
            is_error,
        } => AgentDeliveryEvent::ToolResult {
            name: name.clone(),
            id: id.clone(),
            summary: summary.clone(),
            is_error: *is_error,
        },
        AgentUiEvent::Done => AgentDeliveryEvent::Done,
        AgentUiEvent::Error(message) => AgentDeliveryEvent::Error(message.clone()),
    }
}

/// Drain a single agent UI event.
///
/// Voice-assistive delivery: each event is published to the process-global
/// delivery broadcast (`crate::agent_delivery`) so the bridge can forward it onto
/// the SwiftUI AgentChat listener — this replaces the removed legacy AppKit
/// overlay sink. Consuming `ui_rx` here is also what advances `AgentSession::send`
/// to completion (the channel is bounded). Debug logging of tool activity stays;
/// disk persistence still happens in `run_agent_send_path` after the drain.
async fn apply_agent_ui_event(event: AgentUiEvent) {
    if matches!(event, AgentUiEvent::Done) {
        info!(
            target: "codescribe::agent_delivery",
            "w10a_turn_done"
        );
    }
    crate::agent_delivery::publish_agent_delivery_event(agent_ui_event_to_delivery(&event));
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

fn normalize_assistive_thread_text(text: &str) -> Option<String> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn deliver_runtime_thread(runtime: &AgentRuntime) -> Result<ThreadDeliveryReceipt> {
    let (provider, model) = lane_truth::assistive_identity(&Config::load());
    ThreadDeliveryGateway::new()?.deliver(runtime_delivery_input(
        runtime,
        provider.as_str().to_string(),
        model,
        Utc::now(),
    ))
}

/// Canonical mapping from live runtime state to a delivery input. Shared by the
/// production gateway path and the continuity tests so both persist through the
/// exact same shape.
fn runtime_delivery_input(
    runtime: &AgentRuntime,
    provider: String,
    model: String,
    now: DateTime<Utc>,
) -> ThreadDeliveryInput {
    let messages = runtime
        .session
        .messages()
        .iter()
        .map(|message| {
            let mut persisted = ThreadMessage::from(message);
            if message.timestamp.is_none() {
                persisted.timestamp = now;
            }
            persisted
        })
        .collect::<Vec<_>>();

    ThreadDeliveryInput {
        backend_id: runtime.thread_store_id.clone(),
        messages,
        provider,
        model,
        source: ThreadDeliverySource::VoiceAssistive,
        mode: "assistive".to_string(),
        tags: vec!["agent".to_string(), "overlay".to_string()],
        timestamp: now,
    }
}

fn legacy_assistive_delivery_input(
    user_text: &str,
    assistant_text: &str,
    backend_id: String,
    now: DateTime<Utc>,
    model: String,
) -> Option<ThreadDeliveryInput> {
    let user_text = normalize_assistive_thread_text(user_text)?;
    let assistant_text = normalize_assistive_thread_text(assistant_text)?;
    let metadata = Some(json!({"source":"legacy-fallback"}));

    Some(ThreadDeliveryInput {
        backend_id,
        messages: vec![
            ThreadMessage {
                role: "user".to_string(),
                content: vec![json!({"type":"text","text":user_text})],
                timestamp: now,
                metadata: metadata.clone(),
            },
            ThreadMessage {
                role: "assistant".to_string(),
                content: vec![json!({"type":"text","text":assistant_text})],
                timestamp: now,
                metadata,
            },
        ],
        provider: "legacy-formatter".to_string(),
        model,
        source: ThreadDeliverySource::LegacyFallback,
        mode: "assistive".to_string(),
        tags: vec![
            "agent".to_string(),
            "overlay".to_string(),
            "fallback".to_string(),
        ],
        timestamp: now,
    })
}

fn deliver_legacy_assistive_thread_with_gateway(
    gateway: &ThreadDeliveryGateway,
    user_text: &str,
    assistant_text: &str,
    backend_id: String,
    now: DateTime<Utc>,
    model: String,
) -> Result<Option<ThreadDeliveryReceipt>> {
    let Some(input) =
        legacy_assistive_delivery_input(user_text, assistant_text, backend_id, now, model)
    else {
        return Ok(None);
    };

    gateway.deliver(input).map(Some)
}

fn deliver_legacy_assistive_thread(
    user_text: &str,
    assistant_text: &str,
) -> Result<Option<ThreadDeliveryReceipt>> {
    let gateway = ThreadDeliveryGateway::new()?;
    let now = Utc::now();
    let (_, model) = lane_truth::assistive_identity(&Config::load());

    deliver_legacy_assistive_thread_with_gateway(
        &gateway,
        user_text,
        assistant_text,
        ThreadStore::generate_id(),
        now,
        model,
    )
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
pub(super) fn build_image_attachments_from_text(
    text: &str,
) -> (String, Vec<ImageAttachment>, Vec<String>) {
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
    text: String,
    stream_options: StreamOptions,
) -> Result<AgentSendOutcome> {
    run_agent_send_path_with_persist(runtime_state, text, stream_options, deliver_runtime_thread)
        .await
}

async fn run_agent_send_path_with_persist<P, Delivery>(
    runtime_state: &mut AgentRuntimeState,
    text: String,
    mut stream_options: StreamOptions,
    persist_runtime: P,
) -> Result<AgentSendOutcome>
where
    P: FnOnce(&AgentRuntime) -> Result<Delivery>,
{
    let (runtime, recovered_from_degraded) = match runtime_state.ensure_runtime() {
        Ok(state) => state,
        Err(error) => {
            runtime_state.mark_runtime_degraded("runtime_init_failed");
            return Err(error).context("Agent runtime unavailable");
        }
    };
    let _ = recovered_from_degraded;

    if runtime.reset_chain_on_next_send {
        stream_options.reset_chain = true;
        runtime.reset_chain_on_next_send = false;
    }

    let send_result = {
        // Correlation id for the SwiftUI store (disjoint from its per-thread
        // UUID). Captured before the mutable session/ui_rx split so the borrow of
        // `runtime.thread_store_id` does not overlap the mutable field borrows.
        let thread_store_id = runtime.thread_store_id.clone();
        let messages_before_turn = runtime.session.messages().to_vec();
        let mut cancellation = register_agent_delivery_turn(&thread_store_id);
        let (session, ui_rx, reset_chain_on_next_send) = (
            &mut runtime.session,
            &mut runtime.ui_rx,
            &mut runtime.reset_chain_on_next_send,
        );
        let (user_text, image_attachments, dropped_images) =
            build_image_attachments_from_text(&text);
        // Open the turn on the SwiftUI chat before streaming: the listener inserts
        // a You-bubble (user_text) + assistant placeholder, then fills it from the
        // deltas below. `user_text` is the attachment-marker-stripped transcript,
        // so the bubble shows the spoken text, not the internal attachment block.
        // W10-A runtime receipt: log before publish so installed-app probes can
        // prove reveal_ts < done_ts (Swift logs w10a_reveal_* on the same turn).
        info!(
            target: "codescribe::agent_delivery",
            "w10a_turn_started thread_id={} user_chars={}",
            thread_store_id,
            user_text.chars().count()
        );
        crate::agent_delivery::publish_agent_delivery_event(AgentDeliveryEvent::TurnStarted {
            thread_id: thread_store_id.clone(),
            user_text: user_text.clone(),
        });
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
        enum SendCompletion {
            Finished(Result<()>),
            Cancelled,
        }

        // Scope the pinned send future tightly: cancellation must drop its
        // mutable session borrow before we can restore the pre-turn snapshot.
        let completion = {
            let send_future = session.send(user_text, image_attachments, &stream_options);
            tokio::pin!(send_future);
            loop {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => break SendCompletion::Cancelled,
                    result = &mut send_future => break SendCompletion::Finished(result),
                    maybe_event = ui_rx.recv() => {
                        match maybe_event {
                            Some(event) => {
                                if matches!(event, AgentUiEvent::Done | AgentUiEvent::Error(_)) {
                                    let _ = cancellation.finish();
                                }
                                apply_agent_ui_event(event).await;
                            }
                            None => break SendCompletion::Finished(Err(anyhow::anyhow!("Agent UI event channel closed"))),
                        }
                    }
                }
            }
        };

        match completion {
            SendCompletion::Cancelled => {
                // Dropping `send_future` at this branch aborts provider polling or
                // an in-flight tool at its current await. Restore the exact local
                // history snapshot, reset the provider chain on the next turn,
                // discard queued late UI events, then emit one keyed terminal.
                session.restore_messages(messages_before_turn);
                *reset_chain_on_next_send = true;
                while ui_rx.try_recv().is_ok() {}
                let _ = cancellation.finish();
                crate::agent_delivery::publish_agent_delivery_event(
                    AgentDeliveryEvent::Cancelled {
                        thread_id: thread_store_id,
                    },
                );
                return Ok(AgentSendOutcome::Cancelled);
            }
            SendCompletion::Finished(result) => {
                // Close the registry entry under the same mutex used by the
                // Swift-callable cancel path. If Stop won after `send()` became
                // ready but before this branch ran, cancellation still owns the
                // terminal and queued Done/tool events must not leak through.
                if cancellation.finish() {
                    session.restore_messages(messages_before_turn);
                    *reset_chain_on_next_send = true;
                    while ui_rx.try_recv().is_ok() {}
                    crate::agent_delivery::publish_agent_delivery_event(
                        AgentDeliveryEvent::Cancelled {
                            thread_id: thread_store_id,
                        },
                    );
                    return Ok(AgentSendOutcome::Cancelled);
                }
                while let Ok(event) = ui_rx.try_recv() {
                    if matches!(event, AgentUiEvent::Done | AgentUiEvent::Error(_)) {
                        let _ = cancellation.finish();
                    }
                    apply_agent_ui_event(event).await;
                }
                let _ = cancellation.finish();
                result
            }
        }
    };

    match send_result {
        Ok(()) => {
            if let Err(error) = persist_runtime(runtime) {
                warn!("Failed to persist agent thread: {}", error);
            }
            Ok(AgentSendOutcome::Completed)
        }
        Err(error) => {
            if !agent_send_error_allows_legacy_fallback(&error) {
                return Ok(AgentSendOutcome::Completed);
            }
            // P1.7: distinguish a transient provider blip (conversation still
            // valid -> keep messages, reset chain) from a hard failure (drop the
            // runtime). Both still mark the UI degraded and fall back to legacy.
            if agent_send_error_is_transient(&error) {
                runtime_state.mark_runtime_degraded_preserving_context("send_transient_failure");
            } else {
                runtime_state.mark_runtime_degraded("send_hard_failure");
            }
            Err(error).context("AgentSession send failed")
        }
    }
}

/// Map a legacy formatter result to the assistant text that should be
/// persisted, if any.
///
/// A `Failed` status carries no real assistant content (previously it was
/// surfaced only as the "AI Failed" sentinel). A failed formatting attempt is
/// NOT a conversation and must not be persisted (operator decision
/// 2026-07-06): a dead API key would otherwise land a junk "AI Failed" thread
/// on disk for every retry, producing 3-4 duplicate garbage threads per
/// utterance. `Skipped` likewise has nothing to persist. Only genuine output
/// (`Applied` / `AiNoop`, i.e. partial or full success) is persisted, exactly
/// as before.
fn legacy_fallback_assistant_text(
    status: crate::ai_formatting::AiFormatStatus,
    text: String,
) -> Option<String> {
    use crate::ai_formatting::AiFormatStatus;
    match status {
        AiFormatStatus::Applied | AiFormatStatus::AiNoop => Some(text),
        AiFormatStatus::Failed | AiFormatStatus::Skipped => None,
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

    let status = result.status;
    let assistant_text = legacy_fallback_assistant_text(status, result.text);
    if assistant_text.is_none() && status == crate::ai_formatting::AiFormatStatus::Failed {
        warn!(
            "Legacy formatter failed; skipping thread persist for this attempt (a failed attempt is not a conversation)"
        );
    }
    assistant_text
}

async fn run_agent_send_with_fallback(
    runtime_state: &Arc<TokioMutex<AgentRuntimeState>>,
    text: String,
    whisper_language: crate::config::Language,
    ai_assistive_max_tokens: i32,
    use_assistive_persona: bool,
) {
    let _send_guard = AgentSendInFlightGuard::new();
    let stream_options = build_agent_stream_options(ai_assistive_max_tokens, use_assistive_persona);
    let agent_result = {
        let mut guard = runtime_state.lock().await;
        run_agent_send_path(&mut guard, text.clone(), stream_options).await
    };

    match agent_result {
        Ok(AgentSendOutcome::Completed) => {}
        Ok(AgentSendOutcome::Cancelled) => {
            info!("Voice-assistive Agent turn cancelled; skipping fallback and persistence");
        }
        Err(error) => {
            warn!("Agent fallback triggered: reason={}", error);
            warn!(
                "Agent runtime failed, switching this response to legacy fallback: {}",
                error
            );
            debug!("Legacy fallback input length: {}", text.len());
            let fallback_assistant_text = run_legacy_send_path(&text, whisper_language).await;
            if let Some(assistant_text) = fallback_assistant_text {
                match deliver_legacy_assistive_thread(&text, &assistant_text) {
                    Ok(Some(receipt)) => debug!(
                        backend_thread_id = %receipt.backend_id,
                        message_count = receipt.message_count,
                        "Legacy assistive fallback delivered"
                    ),
                    Ok(None) => {}
                    Err(error) => {
                        warn!("Failed to deliver legacy assistive fallback thread: {error}")
                    }
                }
            }
        }
    }
}

pub(crate) async fn send_assistive_with_agent_runtime_lane(
    text: String,
    whisper_language: crate::config::Language,
    ai_assistive_max_tokens: i32,
    use_assistive_persona: bool,
) {
    let runtime_state = shared_agent_runtime_state();
    run_agent_send_with_fallback(
        &runtime_state,
        text,
        whisper_language,
        ai_assistive_max_tokens,
        use_assistive_persona,
    )
    .await;
}

/// Every recorded mode writes the raw transcript corpus entry once.
pub fn raw_save_enabled(_is_assistive: bool) -> bool {
    true
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
    use codescribe_core::agent::{
        AgentEvent, AgentProvider, ContentBlock, Message, Role, ToolDefinition, ToolResultContent,
    };
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

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

    #[test]
    fn successful_legacy_fallback_delivers_explicit_metadata_and_receipt() {
        let tmp = tempfile::TempDir::new().expect("temp dir should initialize");
        let threads_dir = tmp.path().join("threads");
        let gateway =
            ThreadDeliveryGateway::new_in(&threads_dir).expect("gateway should initialize");
        let now = Utc::now();
        let receipt = deliver_legacy_assistive_thread_with_gateway(
            &gateway,
            "user prompt",
            "assistant reply",
            "t_2026-07-19_legacy-fallback".to_string(),
            now,
            "legacy-test-model".to_string(),
        )
        .expect("legacy fallback delivery should succeed")
        .expect("non-empty fallback should produce a receipt");

        assert!(receipt.created);
        assert_eq!(receipt.message_count, 2);
        assert_eq!(receipt.updated_at, now);
        assert!(receipt.first_exchange);
        assert!(receipt.title_eligible);

        let store = ThreadStore::new_in(&threads_dir).expect("store should reopen");
        let thread = store
            .load_thread(&receipt.backend_id)
            .expect("delivered fallback should load");
        assert_eq!(thread.provider, "legacy-formatter");
        assert_eq!(thread.model, "legacy-test-model");
        assert!(thread.tags.iter().any(|tag| tag == "fallback"));
        assert_eq!(thread.messages.len(), 2);
        assert_eq!(thread.messages[0].role, "user");
        assert_eq!(thread.messages[0].content[0]["type"], "text");
        assert_eq!(thread.messages[0].content[0]["text"], "user prompt");
        assert_eq!(
            thread.messages[0].metadata,
            Some(json!({"source":"legacy-fallback"}))
        );
        assert_eq!(thread.messages[1].role, "assistant");
        assert_eq!(thread.messages[1].content[0]["type"], "text");
        assert_eq!(thread.messages[1].content[0]["text"], "assistant reply");
    }

    #[test]
    fn legacy_fallback_skips_persist_on_failed_status() {
        use crate::ai_formatting::AiFormatStatus;

        // Failed: the formatter produced no real assistant content (dead API
        // key -> "AI Failed"). Nothing to persist, so no thread is written and
        // no messages are built. This is the regression guard for the ~12 junk
        // "AI Failed" threads (incl. 3-4 duplicates per utterance) the operator
        // saw after a dead key drove every retry through the legacy fallback.
        assert_eq!(
            legacy_fallback_assistant_text(AiFormatStatus::Failed, "AI Failed".to_string()),
            None
        );

        // Skipped: also nothing to persist.
        assert_eq!(
            legacy_fallback_assistant_text(AiFormatStatus::Skipped, String::new()),
            None
        );

        // Applied / AiNoop: genuine output (partial or full success) is still
        // persisted exactly as before.
        assert_eq!(
            legacy_fallback_assistant_text(AiFormatStatus::Applied, "formatted reply".to_string()),
            Some("formatted reply".to_string())
        );
        assert_eq!(
            legacy_fallback_assistant_text(AiFormatStatus::AiNoop, "verbatim reply".to_string()),
            Some("verbatim reply".to_string())
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
            reset_chain_on_next_send: false,
        }
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

    /// The per-turn generation machinery is removed: ordinary consecutive
    /// ensures reuse the live runtime as-is — no identity rotation, no history
    /// reset, no rehydration attempt.
    #[test]
    fn test_runtime_generation_machinery_removed_ordinary_ensures_reuse_runtime() {
        let mut runtime_state = AgentRuntimeState {
            runtime: Some(runtime_with_thread_id("thread_existing")),
            thread_store_id: Some("thread_existing".to_string()),
            runtime_degraded: false,
        };
        let init_calls = AtomicUsize::new(0);

        for _ in 0..2 {
            let (runtime, recovered) = runtime_state
                .ensure_runtime_with(
                    || {
                        init_calls.fetch_add(1, Ordering::SeqCst);
                        Ok(runtime_with_thread_id("thread_should_not_be_used"))
                    },
                    |_| -> Result<Option<Vec<Message>>> {
                        panic!("a live runtime must never trigger rehydration")
                    },
                )
                .expect("live runtime should be reused on ordinary consecutive sends");
            assert_eq!(runtime.thread_store_id, "thread_existing");
            assert!(!recovered);
        }

        assert_eq!(init_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            runtime_state.thread_store_id.as_deref(),
            Some("thread_existing")
        );
    }

    #[test]
    fn test_runtime_recovery_clears_degraded_flag_on_reinit() {
        let mut runtime_state = AgentRuntimeState {
            runtime: None,
            thread_store_id: Some("thread_stable".to_string()),
            runtime_degraded: true,
        };
        let init_calls = AtomicUsize::new(0);

        let (runtime, recovered) = runtime_state
            .ensure_runtime_with(
                || {
                    init_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(runtime_with_thread_id("thread_freshly_minted"))
                },
                |_| Ok(None),
            )
            .expect("runtime should reinitialize after degraded state");

        assert_eq!(init_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            runtime.thread_store_id, "thread_stable",
            "rebuild must rejoin the durable thread id, not the freshly minted one"
        );
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
            reset_chain_on_next_send: false,
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
            thread_store_id: Some("thread_transient".to_string()),
            runtime_degraded: false,
        };

        let transient = anyhow::anyhow!("Failed to start 'openai' streaming")
            .context("connection reset by peer");
        assert!(
            agent_send_error_is_transient(&transient),
            "connection-reset error must classify as transient"
        );

        let newly_degraded =
            runtime_state.mark_runtime_degraded_preserving_context("test_transient_failure");
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

    /// Counterpart: a hard (non-transient) failure drops the runtime entirely —
    /// but never the durable thread identity.
    #[test]
    fn hard_degrade_drops_runtime_on_non_transient() {
        let mut runtime_state = AgentRuntimeState {
            runtime: Some(seed_completed_runtime("thread_hard")),
            thread_store_id: Some("thread_hard".to_string()),
            runtime_degraded: false,
        };

        let hard = anyhow::anyhow!("Agent runtime was not initialized");
        assert!(
            !agent_send_error_is_transient(&hard),
            "init failure must NOT classify as transient"
        );

        runtime_state.mark_runtime_degraded("test_hard_failure");
        assert!(
            runtime_state.runtime.is_none(),
            "hard degrade must drop the runtime"
        );
        assert!(runtime_state.runtime_degraded);
        assert_eq!(
            runtime_state.thread_store_id.as_deref(),
            Some("thread_hard"),
            "durable thread identity must survive runtime = None"
        );
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
        std::fs::write(&img, b"\x89PNG\r\n\x1a\nfake").expect("test: write fake image");
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
            std::fs::write(&p, b"\x89PNG\r\n\x1a\nfake")
                .expect("test: write fake image for overflow");
            lines.push_str(&format!("- {}\n", p.display()));
        }
        let (_cleaned, images, dropped) = build_image_attachments_from_text(&lines);

        // Cap honored, overflow surfaced (not silently dropped).
        assert_eq!(images.len(), MAX_AGENT_VISION_IMAGES);
        assert_eq!(dropped.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Voice-assistive delivery: UI-event → delivery-event mapping + publish ──

    #[test]
    fn agent_ui_event_maps_to_delivery_event_one_to_one() {
        assert_eq!(
            agent_ui_event_to_delivery(&AgentUiEvent::TextDelta("hi".into())),
            AgentDeliveryEvent::TextDelta("hi".into())
        );
        assert_eq!(
            agent_ui_event_to_delivery(&AgentUiEvent::TextDone("done".into())),
            AgentDeliveryEvent::TextDone("done".into())
        );
        assert_eq!(
            agent_ui_event_to_delivery(&AgentUiEvent::ReasoningDelta("r".into())),
            AgentDeliveryEvent::ReasoningDelta("r".into())
        );
        assert_eq!(
            agent_ui_event_to_delivery(&AgentUiEvent::ToolExecuting {
                name: "grep".into(),
                id: "1".into(),
            }),
            AgentDeliveryEvent::ToolExecuting {
                name: "grep".into(),
                id: "1".into(),
            }
        );
        assert_eq!(
            agent_ui_event_to_delivery(&AgentUiEvent::ToolResult {
                name: "grep".into(),
                id: "1".into(),
                summary: "2 hits".into(),
                is_error: false,
            }),
            AgentDeliveryEvent::ToolResult {
                name: "grep".into(),
                id: "1".into(),
                summary: "2 hits".into(),
                is_error: false,
            }
        );
        assert_eq!(
            agent_ui_event_to_delivery(&AgentUiEvent::Done),
            AgentDeliveryEvent::Done
        );
        assert_eq!(
            agent_ui_event_to_delivery(&AgentUiEvent::Error("boom".into())),
            AgentDeliveryEvent::Error("boom".into())
        );
    }

    #[tokio::test]
    async fn apply_agent_ui_event_publishes_to_delivery_broadcast() {
        use crate::agent_delivery::{AgentDeliveryEvent, subscribe_agent_delivery};
        use tokio::sync::broadcast::error::RecvError;

        // Unique payload so a concurrent test on the shared global broadcast can
        // never satisfy this matcher.
        let marker = "apply_agent_ui_event_publishes_to_delivery_broadcast";
        let mut rx = subscribe_agent_delivery();
        apply_agent_ui_event(AgentUiEvent::TextDone(marker.into())).await;

        let mut found = None;
        for _ in 0..1024 {
            match rx.recv().await {
                Ok(AgentDeliveryEvent::TextDone(text)) if text == marker => {
                    found = Some(text);
                    break;
                }
                Ok(_) | Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => panic!("delivery channel closed unexpectedly"),
            }
        }
        assert_eq!(found.as_deref(), Some(marker));
    }

    struct ScriptedControllerProvider {
        scripts: Arc<StdMutex<VecDeque<Vec<AgentEvent>>>>,
        reset_chain_flags: Arc<StdMutex<Vec<bool>>>,
        /// Full provider-call inputs, recorded so continuity tests can prove the
        /// second call replays prior history instead of just the new message.
        seen_inputs: Arc<StdMutex<Vec<Vec<Message>>>>,
    }

    #[async_trait]
    impl AgentProvider for ScriptedControllerProvider {
        async fn stream(
            &self,
            messages: &[Message],
            _tools: &[ToolDefinition],
            options: &StreamOptions,
        ) -> anyhow::Result<mpsc::Receiver<AgentEvent>> {
            self.seen_inputs
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(messages.to_vec());
            self.reset_chain_flags
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(options.reset_chain);
            let events = self
                .scripts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .pop_front()
                .context("scripted controller provider exhausted")?;
            let (tx, rx) = mpsc::channel(events.len().max(1));
            tokio::spawn(async move {
                for event in events {
                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            });
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
            "scripted-controller-provider"
        }
    }

    async fn wait_for_flag(flag: &AtomicBool) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !flag.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("test flag should be set before timeout");
    }

    /// Serializes every test that drives `run_agent_send_path_with_persist`:
    /// the send path publishes un-keyed `Done` terminals to the process-global
    /// delivery broadcast, so concurrent send-path tests would leak terminals
    /// into each other's subscriptions.
    static SEND_PATH_BROADCAST_LOCK: TokioMutex<()> = TokioMutex::const_new(());

    #[tokio::test]
    async fn voice_cancel_drops_slow_tool_restores_history_skips_persist_and_recovers() {
        use crate::agent_delivery::{
            AgentDeliveryEvent, cancel_agent_delivery_turn, subscribe_agent_delivery,
        };

        let _broadcast_guard = SEND_PATH_BROADCAST_LOCK.lock().await;
        let thread_id = "controller_voice_cancel_recovery";
        let tool_started = Arc::new(AtomicBool::new(false));
        let side_effect = Arc::new(AtomicBool::new(false));
        let reset_chain_flags = Arc::new(StdMutex::new(Vec::new()));
        let scripts = Arc::new(StdMutex::new(VecDeque::from([
            vec![
                AgentEvent::TextDelta("partial".to_string()),
                AgentEvent::ToolCallReady {
                    id: "slow-call".to_string(),
                    name: "slow_side_effect".to_string(),
                    arguments: json!({}),
                },
                AgentEvent::ResponseDone {
                    response_id: Some("cancelled-response".to_string()),
                    clean: true,
                },
            ],
            vec![
                AgentEvent::TextDone("recovered".to_string()),
                AgentEvent::ResponseDone {
                    response_id: Some("recovered-response".to_string()),
                    clean: true,
                },
            ],
        ])));

        let mut tools = ToolRegistry::new();
        let tool_started_for_handler = Arc::clone(&tool_started);
        let side_effect_for_handler = Arc::clone(&side_effect);
        tools
            .register(
                ToolDefinition {
                    name: "slow_side_effect".to_string(),
                    description: "delayed observable side effect".to_string(),
                    input_schema: json!({"type": "object", "properties": {}}),
                },
                Box::new(move |_input| {
                    let tool_started = Arc::clone(&tool_started_for_handler);
                    let side_effect = Arc::clone(&side_effect_for_handler);
                    Box::pin(async move {
                        tool_started.store(true, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        side_effect.store(true, Ordering::SeqCst);
                        vec![ToolResultContent::Text("side effect fired".to_string())]
                    })
                }),
            )
            .expect("slow tool should register");

        let (ui_tx, ui_rx) = mpsc::channel(32);
        let mut session = AgentSession::new(
            Box::new(ScriptedControllerProvider {
                scripts: Arc::clone(&scripts),
                reset_chain_flags: Arc::clone(&reset_chain_flags),
                seen_inputs: Arc::new(StdMutex::new(Vec::new())),
            }),
            Arc::new(tools),
            ui_tx,
        );
        session.restore_messages(vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Text("prior successful turn".to_string())],
        )]);
        let mut state = AgentRuntimeState {
            runtime: Some(AgentRuntime {
                session,
                ui_rx,
                thread_store_id: thread_id.to_string(),
                reset_chain_on_next_send: false,
            }),
            thread_store_id: Some(thread_id.to_string()),
            runtime_degraded: false,
        };
        let persist_count = Arc::new(AtomicUsize::new(0));
        let first_persist_count = Arc::clone(&persist_count);
        let mut delivery = subscribe_agent_delivery();

        let driven = tokio::spawn(async move {
            let result = run_agent_send_path_with_persist(
                &mut state,
                "cancel this".to_string(),
                test_stream_options(),
                move |_runtime| {
                    first_persist_count.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await;
            (state, result)
        });

        wait_for_flag(&tool_started).await;
        assert!(
            cancel_agent_delivery_turn(thread_id),
            "registered voice turn should cancel without acquiring runtime state"
        );
        let (mut state, result) = driven.await.expect("controller task should not panic");
        assert_eq!(
            result.expect("cancel should be a normal outcome"),
            AgentSendOutcome::Cancelled
        );
        assert_eq!(persist_count.load(Ordering::SeqCst), 0);

        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        assert!(
            !side_effect.load(Ordering::SeqCst),
            "dropping the slow tool future must prevent its later side effect"
        );
        let cancelled_runtime = state
            .runtime
            .as_ref()
            .expect("runtime should survive cancel");
        assert_eq!(cancelled_runtime.session.messages().len(), 1);
        assert!(cancelled_runtime.reset_chain_on_next_send);
        assert!(
            !cancel_agent_delivery_turn(thread_id),
            "cancelled turn must clean its registry token"
        );

        let mut cancelled_terminals = 0;
        let mut successful_or_error_terminals = 0;
        while let Ok(event) = delivery.try_recv() {
            if matches!(
                event,
                AgentDeliveryEvent::Cancelled { thread_id: ref id } if id == thread_id
            ) {
                cancelled_terminals += 1;
            } else if matches!(
                event,
                AgentDeliveryEvent::Done | AgentDeliveryEvent::Error(_)
            ) {
                successful_or_error_terminals += 1;
            }
        }
        assert_eq!(
            cancelled_terminals, 1,
            "voice cancel emits one keyed terminal"
        );
        assert_eq!(
            successful_or_error_terminals, 0,
            "cancelled voice turn must not also emit Done or Error"
        );

        let second_persist_count = Arc::clone(&persist_count);
        let outcome = run_agent_send_path_with_persist(
            &mut state,
            "try again".to_string(),
            test_stream_options(),
            move |_runtime| {
                second_persist_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .expect("next turn should succeed");
        assert_eq!(outcome, AgentSendOutcome::Completed);
        assert_eq!(persist_count.load(Ordering::SeqCst), 1);

        let recovered_runtime = state.runtime.as_ref().expect("runtime should remain live");
        assert_eq!(recovered_runtime.session.messages().len(), 3);
        assert!(recovered_runtime.session.messages().iter().any(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Text(text) if text == "recovered"))
        }));
        assert_eq!(
            *reset_chain_flags
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            vec![false, true],
            "the recovery turn must clear provider-owned chain state before replay"
        );
    }

    fn test_stream_options() -> StreamOptions {
        StreamOptions {
            model: String::new(),
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            reset_chain: false,
        }
    }

    // ── Voice runtime identity and history continuity (W1-A) ────────────────

    fn scripted_runtime(
        thread_store_id: &str,
        scripts: Arc<StdMutex<VecDeque<Vec<AgentEvent>>>>,
        seen_inputs: Arc<StdMutex<Vec<Vec<Message>>>>,
    ) -> AgentRuntime {
        let (ui_tx, ui_rx) = mpsc::channel(32);
        let session = AgentSession::new(
            Box::new(ScriptedControllerProvider {
                scripts,
                reset_chain_flags: Arc::new(StdMutex::new(Vec::new())),
                seen_inputs,
            }),
            Arc::new(ToolRegistry::new()),
            ui_tx,
        );
        AgentRuntime {
            session,
            ui_rx,
            thread_store_id: thread_store_id.to_string(),
            reset_chain_on_next_send: false,
        }
    }

    fn completed_turn_script(assistant_text: &str, response_id: &str) -> Vec<AgentEvent> {
        vec![
            AgentEvent::TextDone(assistant_text.to_string()),
            AgentEvent::ResponseDone {
                response_id: Some(response_id.to_string()),
                clean: true,
            },
        ]
    }

    fn assert_single_controller_thread_artifact(threads_dir: &std::path::Path) {
        let thread_files = std::fs::read_dir(threads_dir)
            .expect("threads dir should list")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
                    && path.file_name().is_some_and(|name| name != "index.json")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            thread_files.len(),
            1,
            "one voice conversation must leave exactly one thread JSON artifact"
        );
        let index_json = std::fs::read_to_string(threads_dir.join("index.json"))
            .expect("index.json should exist");
        let index: serde_json::Value =
            serde_json::from_str(&index_json).expect("index.json should parse");
        assert_eq!(
            index["threads"]
                .as_array()
                .expect("index should hold a threads array")
                .len(),
            1,
            "one voice conversation must leave exactly one index row"
        );
    }

    /// Preserve the real W2-A ThreadStore output when the verifier explicitly
    /// provides an evidence directory. Normal test runs remain hermetic.
    fn export_w2_delivery_artifacts(
        threads_dir: &std::path::Path,
        receipts: &[ThreadDeliveryReceipt],
        persisted: &codescribe_core::agent::Thread,
    ) {
        let Some(artifact_dir) = std::env::var_os("CODESCRIBE_W2_ARTIFACT_DIR") else {
            return;
        };
        assert_eq!(receipts.len(), 2, "W2 receipt requires exactly two turns");

        let artifact_dir = std::path::PathBuf::from(artifact_dir);
        std::fs::create_dir_all(&artifact_dir).expect("W2 artifact dir should initialize");
        let thread_source = threads_dir.join(format!("{}.json", persisted.id));
        let thread_target = artifact_dir.join(format!("thread-{}.json", persisted.id));
        let index_target = artifact_dir.join("index.json");
        std::fs::copy(&thread_source, &thread_target)
            .expect("persisted W2 thread should copy to the evidence directory");
        std::fs::copy(threads_dir.join("index.json"), &index_target)
            .expect("persisted W2 index should copy to the evidence directory");

        let index_json =
            std::fs::read_to_string(&index_target).expect("copied W2 index should remain readable");
        let index: serde_json::Value =
            serde_json::from_str(&index_json).expect("copied W2 index should parse");
        let index_rows = index["threads"]
            .as_array()
            .expect("W2 index should contain thread rows")
            .len();
        let receipt_path = artifact_dir.join("delivery-receipt.json");
        let receipt_json = serde_json::json!({
            "schema": "codescribe.w2-a.delivery.v1",
            "verified_at": Utc::now(),
            "backend_id": persisted.id,
            "thread_file": thread_target,
            "index_file": index_target,
            "index_rows": index_rows,
            "first": {
                "backend_id": receipts[0].backend_id,
                "created": receipts[0].created,
                "message_count": receipts[0].message_count,
                "updated_at": receipts[0].updated_at,
                "first_exchange": receipts[0].first_exchange,
                "title_eligible": receipts[0].title_eligible,
            },
            "second": {
                "backend_id": receipts[1].backend_id,
                "created": receipts[1].created,
                "message_count": receipts[1].message_count,
                "updated_at": receipts[1].updated_at,
                "first_exchange": receipts[1].first_exchange,
                "title_eligible": receipts[1].title_eligible,
            },
            "persisted": {
                "message_count": persisted.messages.len(),
                "updated_at": persisted.updated_at,
                "title": persisted.title,
                "title_is_custom": persisted.title_is_custom,
                "title_is_generated": persisted.title_is_generated,
            },
        });
        std::fs::write(
            &receipt_path,
            serde_json::to_vec_pretty(&receipt_json).expect("W2 receipt should serialize"),
        )
        .expect("W2 receipt should write");
        println!(
            "w2_delivery_artifacts receipt={} thread={} index={}",
            receipt_path.display(),
            thread_target.display(),
            index_target.display()
        );
    }

    /// Two ordinary successful turns share one backend thread: same id, one
    /// disk artifact, one index row, monotonically growing message count, and a
    /// strictly newer `updated_at` on the second delivery.
    #[tokio::test]
    async fn voice_runtime_continuity() {
        let _broadcast_guard = SEND_PATH_BROADCAST_LOCK.lock().await;
        let tmp = tempfile::TempDir::new().expect("temp dir should initialize");
        let threads_dir = tmp.path().join("threads");
        let gateway =
            ThreadDeliveryGateway::new_in(&threads_dir).expect("gateway should initialize");

        let scripts = Arc::new(StdMutex::new(VecDeque::from([
            completed_turn_script("first answer", "resp-first"),
            completed_turn_script("second answer", "resp-second"),
        ])));
        let seen_inputs = Arc::new(StdMutex::new(Vec::new()));
        let mut state = AgentRuntimeState {
            runtime: Some(scripted_runtime(
                "t_test_continuity",
                Arc::clone(&scripts),
                Arc::clone(&seen_inputs),
            )),
            thread_store_id: Some("t_test_continuity".to_string()),
            runtime_degraded: false,
        };

        let mut receipts: Vec<ThreadDeliveryReceipt> = Vec::new();
        for text in ["first question", "second question"] {
            let outcome = run_agent_send_path_with_persist(
                &mut state,
                text.to_string(),
                test_stream_options(),
                |runtime| {
                    let receipt = gateway.deliver(runtime_delivery_input(
                        runtime,
                        "test-provider".to_string(),
                        "test-model".to_string(),
                        Utc::now(),
                    ))?;
                    receipts.push(receipt);
                    Ok(())
                },
            )
            .await
            .expect("ordinary turn should complete");
            assert_eq!(outcome, AgentSendOutcome::Completed);
        }

        assert_eq!(receipts.len(), 2, "both turns must persist");
        assert_eq!(receipts[0].backend_id, "t_test_continuity");
        assert_eq!(
            receipts[1].backend_id, "t_test_continuity",
            "ordinary turns must never rotate thread identity"
        );
        assert!(receipts[0].created);
        assert!(
            !receipts[1].created,
            "the second ordinary turn must upsert the same thread, not create a new one"
        );
        assert_eq!(receipts[0].message_count, 2);
        assert_eq!(
            receipts[1].message_count, 4,
            "message count must grow monotonically across turns"
        );
        assert!(
            receipts[1].updated_at > receipts[0].updated_at,
            "the second delivery must carry a newer updated_at"
        );

        assert_eq!(state.thread_store_id.as_deref(), Some("t_test_continuity"));
        let runtime = state.runtime.as_ref().expect("runtime should stay live");
        assert_eq!(runtime.thread_store_id, "t_test_continuity");
        assert_eq!(
            runtime.session.messages().len(),
            4,
            "in-memory history must accumulate, never reset between ordinary turns"
        );

        assert_single_controller_thread_artifact(&threads_dir);
    }

    /// Hard degrade drops the runtime but not the durable identity: the rebuilt
    /// runtime rejoins the same backend thread, restores the persisted history
    /// before the next provider call, and the second call replays the first
    /// exchange instead of starting over.
    #[tokio::test]
    async fn hard_degrade_rehydrates_same_thread() {
        let _broadcast_guard = SEND_PATH_BROADCAST_LOCK.lock().await;
        let tmp = tempfile::TempDir::new().expect("temp dir should initialize");
        let threads_dir = tmp.path().join("threads");
        let gateway =
            ThreadDeliveryGateway::new_in(&threads_dir).expect("gateway should initialize");
        let store = ThreadStore::new_in(&threads_dir).expect("store should initialize");

        let first_scripts = Arc::new(StdMutex::new(VecDeque::from([completed_turn_script(
            "first answer",
            "resp-first",
        )])));
        let mut state = AgentRuntimeState {
            runtime: None,
            thread_store_id: None,
            runtime_degraded: false,
        };
        state
            .ensure_runtime_with(
                || {
                    Ok(scripted_runtime(
                        "t_test_stable",
                        Arc::clone(&first_scripts),
                        Arc::new(StdMutex::new(Vec::new())),
                    ))
                },
                |_| Ok(None),
            )
            .expect("first install should succeed");
        let id_before = state
            .thread_store_id
            .clone()
            .expect("install must record the durable identity");
        assert_eq!(id_before, "t_test_stable");

        let mut receipts = Vec::new();
        let outcome = run_agent_send_path_with_persist(
            &mut state,
            "first question".to_string(),
            test_stream_options(),
            |runtime| {
                let receipt = gateway.deliver(runtime_delivery_input(
                    runtime,
                    "test-provider".to_string(),
                    "test-model".to_string(),
                    Utc::now(),
                ))?;
                receipts.push(receipt);
                Ok(())
            },
        )
        .await
        .expect("first turn should complete");
        assert_eq!(outcome, AgentSendOutcome::Completed);

        state.mark_runtime_degraded("test_hard_failure");
        assert!(state.runtime.is_none());
        assert_eq!(
            state.thread_store_id.as_deref(),
            Some(id_before.as_str()),
            "backend thread id must survive runtime = None"
        );

        let second_scripts = Arc::new(StdMutex::new(VecDeque::from([completed_turn_script(
            "second answer",
            "resp-second",
        )])));
        let second_inputs = Arc::new(StdMutex::new(Vec::new()));
        {
            let (runtime, recovered) = state
                .ensure_runtime_with(
                    || {
                        Ok(scripted_runtime(
                            "t_test_freshly_minted",
                            Arc::clone(&second_scripts),
                            Arc::clone(&second_inputs),
                        ))
                    },
                    |thread_store_id| load_thread_messages_from(&store, thread_store_id),
                )
                .expect("recovery rebuild should succeed");
            assert!(recovered);
            assert_eq!(
                runtime.thread_store_id, id_before,
                "recovery must rejoin the durable thread id, never mint a new one"
            );
            assert_eq!(
                runtime.session.messages().len(),
                2,
                "persisted history must be restored before the next provider call"
            );
        }

        let outcome = run_agent_send_path_with_persist(
            &mut state,
            "second question".to_string(),
            test_stream_options(),
            |runtime| {
                let receipt = gateway.deliver(runtime_delivery_input(
                    runtime,
                    "test-provider".to_string(),
                    "test-model".to_string(),
                    Utc::now(),
                ))?;
                receipts.push(receipt);
                Ok(())
            },
        )
        .await
        .expect("recovered turn should complete");
        assert_eq!(outcome, AgentSendOutcome::Completed);
        assert_eq!(state.thread_store_id.as_deref(), Some(id_before.as_str()));
        assert_eq!(receipts.len(), 2, "both recovered turns must persist");
        assert_eq!(receipts[0].backend_id, receipts[1].backend_id);
        assert!(receipts[0].created);
        assert!(!receipts[1].created);
        assert_eq!(receipts[0].message_count, 2);
        assert_eq!(receipts[1].message_count, 4);
        assert!(receipts[1].updated_at > receipts[0].updated_at);

        let inputs = second_inputs
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            inputs.len(),
            1,
            "the rebuilt provider should see exactly one call"
        );
        let second_call_input = &inputs[0];
        assert!(
            second_call_input.len() >= 3,
            "second provider call must replay prior history plus the new user message, got {} message(s)",
            second_call_input.len()
        );
        let replayed_text = second_call_input
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            replayed_text.contains("first question"),
            "replayed input must contain the first user message"
        );
        assert!(
            replayed_text.contains("first answer"),
            "replayed input must contain the first assistant reply"
        );
        assert!(
            replayed_text.contains("second question"),
            "replayed input must contain the new user message"
        );

        let persisted = store
            .load_thread(&id_before)
            .expect("recovered thread should load from disk");
        assert_eq!(
            persisted.messages.len(),
            4,
            "the same thread file must accumulate both turns"
        );
        assert_single_controller_thread_artifact(&threads_dir);
        export_w2_delivery_artifacts(&threads_dir, &receipts, &persisted);
    }

    /// A corrupt/missing ThreadStore artifact must never silently mint a new
    /// thread: identity stays stable, history starts empty, and the lifecycle
    /// logs carry explicit recovery evidence — ids, counts, and classes only,
    /// never prompt/transcript content.
    #[test]
    fn rehydrate_failure_keeps_identity_and_logs_privacy_safe_recovery() {
        struct SharedWriter(Arc<StdMutex<Vec<u8>>>);
        impl std::io::Write for SharedWriter {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let tmp = tempfile::TempDir::new().expect("temp dir should initialize");
        let threads_dir = tmp.path().join("threads");
        let store = ThreadStore::new_in(&threads_dir).expect("store should initialize");
        let corrupt_path = store
            .thread_file_path("t_test_corrupt")
            .expect("thread path should build");
        std::fs::write(&corrupt_path, b"{ this is not valid thread json")
            .expect("corrupt artifact should write");

        let sentinel = "TOP-SECRET-TRANSCRIPT-SENTINEL";
        let mut state = AgentRuntimeState {
            runtime: Some(runtime_with_thread_id("t_test_corrupt")),
            thread_store_id: Some("t_test_corrupt".to_string()),
            runtime_degraded: false,
        };
        state
            .runtime
            .as_mut()
            .expect("runtime is installed")
            .session
            .restore_messages(vec![Message::new(
                Role::User,
                vec![ContentBlock::Text(sentinel.to_string())],
            )]);

        let buffer: Arc<StdMutex<Vec<u8>>> = Arc::new(StdMutex::new(Vec::new()));
        let writer_buffer = Arc::clone(&buffer);
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || SharedWriter(Arc::clone(&writer_buffer)))
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            state.mark_runtime_degraded("test_hard_failure");
            let (runtime, recovered) = state
                .ensure_runtime_with(
                    || Ok(runtime_with_thread_id("t_test_should_be_overridden")),
                    |thread_store_id| load_thread_messages_from(&store, thread_store_id),
                )
                .expect("rebuild should survive a corrupt artifact");
            assert!(recovered);
            assert_eq!(
                runtime.thread_store_id, "t_test_corrupt",
                "identity must stay stable even when rehydration fails"
            );
            assert!(
                runtime.session.messages().is_empty(),
                "failed rehydration continues with empty history on the same thread"
            );
        });
        assert_eq!(state.thread_store_id.as_deref(), Some("t_test_corrupt"));

        let logs = String::from_utf8(
            buffer
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
        )
        .expect("captured logs should be utf8");
        assert!(
            logs.contains("hard_degrade"),
            "hard degrade must log its recovery class: {logs}"
        );
        assert!(
            logs.contains("dropped_message_count=1"),
            "hard degrade must log the dropped in-memory count: {logs}"
        );
        assert!(
            logs.contains("rehydrate_failed"),
            "failed rehydration must be explicit recovery evidence: {logs}"
        );
        assert!(
            logs.contains("t_test_corrupt"),
            "lifecycle logs must carry the thread id transition: {logs}"
        );
        assert!(
            !logs.contains(sentinel),
            "lifecycle logs must never contain prompt/transcript content"
        );
    }
}
