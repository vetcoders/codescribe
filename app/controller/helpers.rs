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
use codescribe_core::config::{Config, RuntimeLlmLane, RuntimeSettingsSnapshot};
use crate::os::hold_badge::{BadgeMode, HoldBadgeConfig, show_hold_badge_with_config};
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
        let persisted_size = Config::load_without_keychain().hold_badge_size;
        show_hold_badge_with_config(HoldBadgeConfig::from_mode_with_base_diameter(
            mode,
            f64::from(persisted_size),
        ));
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
    /// Forward only the incremental `delta` payload — the sink deliberately
    /// discards the rest of the envelope so no caller can smuggle a full preview
    /// snapshot through a channel that promises backspace semantics.
    fn apply(&self, delta: &codescribe_core::pipeline::contracts::TranscriptDelta) {
        route_transcription_delta(&delta.delta);
    }
}

/// Bound on the agent UI event channel. The bound is load-bearing, not just a
/// memory guard: because the channel can fill, draining `ui_rx` is what drives
/// `AgentSession::send` forward (see `run_agent_send_path_with_persist`).
const AGENT_UI_CHANNEL_CAPACITY: usize = 256;

/// Number of agent sends currently in flight, process-wide. A counter rather
/// than a flag so nested/overlapping sends do not clear the state on the first
/// one to finish.
static AGENT_SEND_IN_FLIGHT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Process-global agent runtime, shared by every voice-assistive turn so a
/// conversation keeps one identity and one history across calls. Two layers:
/// the `OnceLock<StdMutex<..>>` is the lazily-built slot, the inner
/// `TokioMutex` serializes the async turns themselves.
static SHARED_AGENT_RUNTIME_STATE: OnceLock<StdMutex<Option<Arc<TokioMutex<AgentRuntimeState>>>>> =
    OnceLock::new();

/// A live agent conversation: the provider session, the UI event channel that
/// must be drained to advance it, and the durable backend thread this runtime is
/// currently bound to.
struct AgentRuntime {
    session: AgentSession,
    ui_rx: mpsc::Receiver<AgentUiEvent>,
    thread_store_id: String,
    /// Soft-degrade / retry flag: next send clears the provider chain and
    /// full-replays local history. User Stop does **not** set this — Stop
    /// restores the pre-turn chain snapshot instead (operator 2026-08-05).
    reset_chain_on_next_send: bool,
}

/// How a send path ended when it did not fail. Cancellation is an `Ok` outcome,
/// not an error: user Stop is a normal exit that must skip persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentSendOutcome {
    /// The turn ran to completion; its thread is eligible for persistence.
    Completed,
    /// The user stopped the turn. Nothing is persisted.
    Cancelled,
}

/// Everything the controller keeps about the agent conversation between turns.
/// The runtime itself is disposable; `thread_store_id` and the degraded flag are
/// what let a dropped runtime be rebuilt onto the same conversation.
#[derive(Default)]
struct AgentRuntimeState {
    runtime: Option<AgentRuntime>,
    /// Durable backend thread identity. Recorded when a runtime is installed and
    /// retained across `runtime = None`, so a rebuilt runtime rejoins the same
    /// thread (and its persisted history) instead of silently starting a new one.
    thread_store_id: Option<String>,
    runtime_degraded: bool,
}

/// RAII marker for "an agent send is running". Tied to a guard rather than
/// hand-written increment/decrement pairs so an early return or a panic on the
/// send path cannot strand the counter above zero.
struct AgentSendInFlightGuard;

impl AgentSendInFlightGuard {
    /// Register one in-flight send for as long as the guard is held.
    fn new() -> Self {
        AGENT_SEND_IN_FLIGHT_COUNT.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for AgentSendInFlightGuard {
    /// Release this send's claim. Nested guards each own exactly one count, so
    /// the flag only clears once the outermost send is done.
    fn drop(&mut self) {
        AGENT_SEND_IN_FLIGHT_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether any agent send is currently running. Read by controller paths that
/// must not interleave with an in-flight turn.
pub(crate) fn is_agent_send_in_flight() -> bool {
    AGENT_SEND_IN_FLIGHT_COUNT.load(Ordering::SeqCst) > 0
}

/// Force the in-flight counter for tests that need a known starting point on a
/// process-global that other tests may have touched.
#[cfg(test)]
pub(super) fn set_agent_send_in_flight_for_test(active: bool) {
    AGENT_SEND_IN_FLIGHT_COUNT.store(if active { 1 } else { 0 }, Ordering::SeqCst);
}

impl AgentRuntimeState {
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
            if let Some(thread_store_id) = &self.thread_store_id {
                // Voice lane has no approval-handler builder, so bind the
                // durable thread id here for thread-context tool dispatch
                // (run-monitor heartbeats re-enter this exact thread).
                runtime
                    .session
                    .bind_execution_thread(thread_store_id.clone());
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

    /// Rebind the assistive conversation to the UI-selected thread (operator
    /// contract 2026-08-13: dictation routes to the thread the user is looking
    /// at; a new thread is only ever minted by an explicit "+ New thread").
    ///
    /// Dropping the runtime on a change deliberately reuses the degrade→rejoin
    /// machinery: the next `ensure_runtime` rebuilds onto the new identity and
    /// rehydrates its persisted history. `None` clears the identity so the next
    /// send mints a fresh thread. Same-target calls are no-ops — the live
    /// runtime and its in-memory history stay untouched.
    fn retarget_thread(&mut self, target: Option<String>) {
        if self.thread_store_id == target {
            return;
        }
        let previous = self.thread_store_id.clone();
        self.runtime = None;
        self.thread_store_id = target;
        info!(
            from = previous.as_deref().unwrap_or("<unassigned>"),
            to = self.thread_store_id.as_deref().unwrap_or("<fresh>"),
            "Assistive lane retargeted to UI-selected thread"
        );
    }
}

/// UI-selected assistive routing target.
///
/// Outer `None`: the Agent UI never published a selection (window never
/// opened) — the lane keeps its legacy behavior of continuing the bound
/// conversation. `Some(None)`: the UI selected a not-yet-persisted thread
/// (explicit "+ New thread") — the next send mints a fresh thread, then the
/// send path syncs the minted identity back here so ONE conscious new-thread
/// press produces one thread, not one per utterance.
static ASSISTIVE_TARGET_THREAD: std::sync::RwLock<Option<Option<String>>> =
    std::sync::RwLock::new(None);

/// Publish the Agent UI's current thread selection as the assistive routing
/// target. Called from the bridge whenever the selection changes.
pub fn set_assistive_target_thread(backend_id: Option<String>) {
    *ASSISTIVE_TARGET_THREAD
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Some(backend_id);
}

/// Snapshot the published routing target, if the UI ever published one.
fn assistive_target_thread() -> Option<Option<String>> {
    ASSISTIVE_TARGET_THREAD
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// The lazily-initialized slot holding the process-global runtime state.
fn shared_agent_runtime_state_slot() -> &'static StdMutex<Option<Arc<TokioMutex<AgentRuntimeState>>>>
{
    SHARED_AGENT_RUNTIME_STATE.get_or_init(|| StdMutex::new(None))
}

/// Hand out the one shared runtime state, creating it on first use. Every voice
/// turn goes through this, which is what makes consecutive utterances one
/// conversation instead of a series of independent sessions.
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

/// Store-injectable half of `rehydrate_thread_messages`, so recovery tests can
/// drive a temp ThreadStore. Distinguishes "no artifact yet" (`Ok(None)`, a
/// normal first-turn state) from a real load failure, which must stay an `Err`
/// so the caller can log it as recovery evidence instead of treating a corrupt
/// thread as an empty one.
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

/// Build a fresh agent runtime: full tool registry under the live permission
/// policy (with hot reload), the configured default provider, and a bounded UI
/// channel. The thread id minted here is provisional — `ensure_runtime_with`
/// overwrites it with the durable identity whenever one already exists.
fn initialize_agent_runtime(assistive_lane: &RuntimeLlmLane) -> Result<AgentRuntime> {
    let mut registry = ToolRegistry::new();
    crate::agent::tools::register_all_tools(&mut registry);
    // B2: same policy load as the UniFFI bridge path — settings.json
    // agent.permissions + legacy tool_grants always-allow keys.
    registry.set_policy(
        codescribe_core::agent::permissions::AgentPermissions::load()
            .with_legacy_grants(codescribe_core::agent::tool_grants::load_granted()),
    );
    registry.enable_policy_hot_reload();

    let provider = crate::agent::create_default_provider(assistive_lane)
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

/// Per-turn provider options. The model comes from live assistive lane truth
/// rather than a cached value, and a non-positive `ai_assistive_max_tokens`
/// resolves to `None` (provider default) instead of a zero-token request.
/// `reset_chain` stays false here: only the retry path overrides it.
fn build_agent_stream_options(
    ai_assistive_max_tokens: i32,
    use_assistive_persona: bool,
    assistive_lane: &RuntimeLlmLane,
) -> StreamOptions {
    let max_tokens = u32::try_from(ai_assistive_max_tokens)
        .ok()
        .filter(|tokens| *tokens > 0);

    StreamOptions {
        model: assistive_lane.model().to_string(),
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
    // Measured Responses/streaming contract facts + the answer-first rule —
    // rides BOTH lanes so a spoken engine question gets substance, not a
    // clarification questionnaire (operator incident 2026-08-14).
    let api_truth = crate::agent::tools::api_truth::responses_api_prompt_section();
    if use_assistive_persona {
        let base = crate::config::get_assistive_prompt();
        format!("{base}\n\n{workspace}\n\n{doctrine}\n\n{api_truth}")
    } else {
        format!(
            "You are the Codescribe agent. Answer and act on the user's spoken request using the available tools when helpful.\n\n{workspace}\n\n{doctrine}\n\n{api_truth}"
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
        // The voice lane has no approval card, so a tool that asks is refused
        // rather than silently allowed. Since native side-effectful tools now
        // reach the gate (review P1-06), reaching this arm is expected for e.g.
        // `type_text` — the operator lifts it durably in
        // Settings → Permissions (`agent.permissions.tools["native:type_text"]`),
        // which outranks the risk default.
        AgentUiEvent::ToolApprovalRequested(request) => AgentDeliveryEvent::Error(format!(
            "Tool approval is unavailable for voice turn {} ({}) — allow it in Settings → Permissions to use it hands-free",
            request.call_id, request.tool
        )),
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
        AgentUiEvent::ToolApprovalRequested(request) => {
            warn!(
                call_id = %request.call_id,
                tool = %request.tool,
                "Voice tool call requires approval; no voice approval broker is connected"
            );
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

/// Production persist hook: stamp the runtime's history with live assistive
/// provider/model identity and upsert it through the delivery gateway. Called
/// after a completed turn only — cancelled turns never reach here.
fn deliver_runtime_thread(
    runtime: &AgentRuntime,
    assistive_lane: &RuntimeLlmLane,
) -> Result<ThreadDeliveryReceipt> {
    ThreadDeliveryGateway::new()?.deliver(runtime_delivery_input(
        runtime,
        assistive_lane.provider().as_str().to_string(),
        assistive_lane.model().to_string(),
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

/// Whether the provider stream has already emitted the terminal UI error.
/// Initialization failures happen before the stream owns a UI turn and need one
/// explicit terminal event at the controller boundary.
fn agent_send_error_was_published(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    message.starts_with("Provider stream error:")
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

/// Production send path: `run_agent_send_path_with_persist` wired to the real
/// ThreadStore delivery.
async fn run_agent_send_path(
    runtime_state: &mut AgentRuntimeState,
    text: String,
    stream_options: StreamOptions,
    assistive_lane: &RuntimeLlmLane,
) -> Result<AgentSendOutcome> {
    run_agent_send_path_with_persist(
        runtime_state,
        text,
        stream_options,
        || initialize_agent_runtime(assistive_lane),
        |runtime| deliver_runtime_thread(runtime, assistive_lane),
    )
    .await
}

/// Drive one agent turn end to end, with an injectable persist hook so tests can
/// observe delivery without a real ThreadStore.
///
/// The loop is a three-way race between the send future, the UI event stream,
/// and user Stop. Draining `ui_rx` is not optional bookkeeping — the channel is
/// bounded, so consuming events is what lets `AgentSession::send` make progress.
///
/// Cancellation has two windows, and both are handled: Stop arriving mid-stream,
/// and Stop landing after `send()` became ready but before this function
/// observed it. Either way the pre-turn history and provider chain are restored
/// (Stop cancels a turn, it does not reset the conversation), queued events are
/// drained so no terminal leaks out after the `Cancelled` one, and persistence is
/// skipped entirely.
///
/// On failure the error is classified before degrading: transient blips keep the
/// runtime and only reset the chain, hard failures drop it.
async fn run_agent_send_path_with_persist<Init, P, Delivery>(
    runtime_state: &mut AgentRuntimeState,
    text: String,
    mut stream_options: StreamOptions,
    initialize_runtime: Init,
    persist_runtime: P,
) -> Result<AgentSendOutcome>
where
    Init: FnOnce() -> Result<AgentRuntime>,
    P: FnOnce(&AgentRuntime) -> Result<Delivery>,
{
    let (runtime, recovered_from_degraded) =
        match runtime_state.ensure_runtime_with(initialize_runtime, rehydrate_thread_messages) {
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
        // Snapshot chain before any mid-turn advance so user Stop can reinstate
        // continuity for a queued follow-up instead of wiping previous_response_id.
        let session_thread_before = runtime.session.thread_id().map(str::to_owned);
        let provider_chain_before = runtime.session.snapshot_response_chain().await;
        let mut cancellation = register_agent_delivery_turn(&thread_store_id);
        let (session, ui_rx) = (&mut runtime.session, &mut runtime.ui_rx);
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
        /// How the select loop below broke. Function-local because it exists
        /// only to carry the loop's verdict past the borrow scope of the pinned
        /// send future — the future must be dropped before the pre-turn
        /// snapshot can be restored.
        enum SendCompletion {
            /// The send future resolved; carries its own success or failure.
            Finished(Result<()>),
            /// User Stop won the race.
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
                // Dropping `send_future` aborts provider polling / in-flight tools.
                // Restore pre-turn history AND chain (do not force full-reset):
                // Stop only cancels the stream; a queued next message keeps continuity.
                session
                    .restore_after_user_stop(
                        messages_before_turn,
                        session_thread_before,
                        provider_chain_before,
                    )
                    .await;
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
                    session
                        .restore_after_user_stop(
                            messages_before_turn,
                            session_thread_before,
                            provider_chain_before,
                        )
                        .await;
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
            if agent_send_error_was_published(&error) {
                return Ok(AgentSendOutcome::Completed);
            }
            // P1.7: distinguish a transient provider blip (conversation still
            // valid -> keep messages, reset chain) from a hard failure (drop the
            // runtime). Both mark the UI degraded; neither creates another route.
            if agent_send_error_is_transient(&error) {
                runtime_state.mark_runtime_degraded_preserving_context("send_transient_failure");
            } else {
                runtime_state.mark_runtime_degraded("send_hard_failure");
            }
            Err(error).context("AgentSession send failed")
        }
    }
}

/// One complete voice-assistive turn on the sole Agent route. Provider or
/// initialization failure is terminal for this turn; it is never replayed by a
/// second formatter authority.
async fn run_agent_send(
    runtime_state: &Arc<TokioMutex<AgentRuntimeState>>,
    assistive_lane: &RuntimeLlmLane,
    text: String,
    ai_assistive_max_tokens: i32,
    use_assistive_persona: bool,
) {
    let _send_guard = AgentSendInFlightGuard::new();
    let stream_options = build_agent_stream_options(
        ai_assistive_max_tokens,
        use_assistive_persona,
        assistive_lane,
    );
    let agent_result = {
        let mut guard = runtime_state.lock().await;
        // Route to the thread the user is looking at (operator contract
        // 2026-08-13). No published selection keeps the bound conversation.
        let fresh_mint_requested = match assistive_target_thread() {
            Some(target) => {
                let fresh = target.is_none();
                guard.retarget_thread(target);
                fresh
            }
            None => false,
        };
        let result = run_agent_send_path(
            &mut guard,
            text,
            stream_options,
            assistive_lane,
        )
        .await;
        if fresh_mint_requested {
            // One conscious "+ New thread" = one thread: adopt the minted
            // identity as the target so the next utterance continues it. The
            // UI's own post-turn refresh republishes the same identity.
            set_assistive_target_thread(guard.thread_store_id.clone());
        }
        result
    };

    match agent_result {
        Ok(AgentSendOutcome::Completed) => {}
        Ok(AgentSendOutcome::Cancelled) => {
            info!("Voice-assistive Agent turn cancelled; skipping persistence");
        }
        Err(error) => {
            warn!("Agent runtime turn failed without alternate route: {error:#}");
            if !agent_send_error_was_published(&error) {
                crate::agent_delivery::publish_agent_delivery_event(
                    AgentDeliveryEvent::Error(error.to_string()),
                );
            }
        }
    }
}

/// Controller entry point for the assistive lane: bind the turn to the shared
/// process-global runtime so consecutive utterances continue one conversation,
/// then run it on the lane sealed by that controller's immutable snapshot.
pub(crate) async fn send_assistive_with_agent_runtime_lane(
    runtime_settings: Arc<RuntimeSettingsSnapshot>,
    text: String,
    _whisper_language: crate::config::Language,
    ai_assistive_max_tokens: i32,
    use_assistive_persona: bool,
) {
    let runtime_state = shared_agent_runtime_state();
    run_agent_send(
        &runtime_state,
        runtime_settings.llm_lanes().assistive(),
        text,
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

/// How the last committed streaming text was sourced (adjudicator truth).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletenessCommitSource {
    /// At least one `UtteranceFinal` landed in this session.
    UtteranceFinal,
    /// `SessionFinalised` sealed the buffer.
    SessionFinalised,
}

impl CompletenessCommitSource {
    /// Stable wire/log tag for this commit source.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UtteranceFinal => "utterance_final",
            Self::SessionFinalised => "session_finalised",
        }
    }
}

/// Engine warning code raised when the Layer 1 tail patch classified an
/// under-commit retranscription and recovered speech it could **not** place on
/// a demonstrably safe anchor (`core::stt::tail_patcher` →
/// `core::pipeline::streaming::session`, W-C / commit `6d7eaa7f`).
///
/// Mirrored as a literal rather than imported: core's canonical
/// `UNDER_COMMIT_WARNING_CODE` is `pub` inside a `pub(crate) mod session`, so it
/// is not nameable from this crate and widening that visibility sits outside
/// this cut's fence. Matched EXACTLY — the sibling `tail_patch_skipped` receipt
/// and any future neighbouring code must not force residual gap fill.
pub(crate) const UNDER_COMMIT_WARNING_CODE: &str = "tail_patch_under_commit";

/// Session telemetry captured from `EngineEvent`s.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionTelemetrySnapshot {
    pub no_speech_reason: Option<String>,
    pub stats: Option<SessionEngineStats>,
    /// Open Preview/Correction without a subsequent UtteranceFinal (pending tail).
    pub pending_tail: bool,
    /// Last adjudicator commit that sealed streaming text, if any.
    pub last_commit_source: Option<CompletenessCommitSource>,
    /// Characters accumulated from UtteranceFinal commits (coverage signal).
    pub committed_chars: usize,
    /// Audio boundary of committed streaming text: the monotonic max `end_ts`
    /// across UtteranceFinal events. Smart-mode stop transcribes only the tail
    /// after this point (append-only doctrine — committed text is immutable).
    pub committed_through_secs: Option<f32>,
    /// Layer 1 escalated an under-commit residual it could not place on a safe
    /// anchor ([`UNDER_COMMIT_WARNING_CODE`]). Monotonic within one session:
    /// once a hole is known no later healthy event may un-know it, because the
    /// speech is already missing from the canvas the stop path is about to
    /// deliver. `Default` starts it false by construction, so
    /// [`reset_session_telemetry`] is the only thing that clears it and a new
    /// recording can never inherit the previous session's residual demand.
    pub residual_required: bool,
}

/// Telemetry handle shared between the engine's event sink and the controller
/// that reads it. A blocking `StdMutex` on purpose: `EventSink::on_event` is
/// sync, and every critical section here is a few field writes.
pub(crate) type SharedSessionTelemetry = Arc<StdMutex<SessionTelemetrySnapshot>>;

/// Empty telemetry for a new session.
pub(crate) fn new_session_telemetry() -> SharedSessionTelemetry {
    Arc::new(StdMutex::new(SessionTelemetrySnapshot::default()))
}

/// Clear telemetry between sessions. Resets the whole snapshot — including the
/// committed audio boundary — so a new recording never inherits the previous
/// session's tail position.
pub(crate) fn reset_session_telemetry(shared: &SharedSessionTelemetry) {
    let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
    *guard = SessionTelemetrySnapshot::default();
}

/// Copy the current telemetry out. Cloned rather than borrowed so controller
/// decisions never read fields while the engine is still writing them.
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
    /// Wrap a shared telemetry handle as an engine event sink.
    pub(crate) fn new(shared: SharedSessionTelemetry) -> Self {
        Self { shared }
    }
}

/// Broadcasts sanitized engine events to IPC subscribers.
pub(crate) struct IpcBroadcastSink {
    tx: broadcast::Sender<IpcEvent>,
}

impl IpcBroadcastSink {
    /// Wrap a broadcast sender as an engine event sink.
    pub(crate) fn new(tx: broadcast::Sender<IpcEvent>) -> Self {
        Self { tx }
    }
}

impl EventSink for IpcBroadcastSink {
    /// Stamp the event and publish it. A send failure is ignored on purpose:
    /// "no IPC subscribers right now" is the normal state, not an engine error,
    /// and telemetry must never be able to stall the pipeline.
    fn on_event(&self, event: &EngineEvent) {
        let ipc_event = IpcEvent {
            timestamp: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            payload: IpcEventPayload::Engine(EngineEventWire::from(event)),
        };
        let _ = self.tx.send(ipc_event);
    }
}

impl EventSink for SessionTelemetrySink {
    /// Fold one engine event into the session snapshot.
    ///
    /// Preview and Correction open a pending tail; only a commit closes it.
    /// `committed_through_secs` advances as a monotonic max so an out-of-order
    /// final cannot rewind the boundary, and non-finite `end_ts` values are
    /// rejected outright — NaN would slip past the `current >= end_ts` guard and
    /// overwrite a valid maximum, silently disabling Smart tail gap-fill for the
    /// rest of the session. Unmatched events are ignored rather than
    /// exhaustively listed, so new engine events cannot break the build here.
    ///
    /// `Warning` is the one event folded by code rather than by variant: only
    /// [`UNDER_COMMIT_WARNING_CODE`] sets `residual_required`, and it sets it
    /// monotonically. Every other warning falls through to the ignore arm.
    fn on_event(&self, event: &EngineEvent) {
        let mut guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            EngineEvent::NoSpeech { reason } => {
                guard.no_speech_reason = Some(reason.clone());
            }
            // Preview / Correction leave an open tail until UtteranceFinal seals it.
            EngineEvent::Preview { .. } | EngineEvent::Correction { .. } => {
                guard.pending_tail = true;
            }
            EngineEvent::UtteranceFinal { text, end_ts, .. } => {
                guard.pending_tail = false;
                guard.last_commit_source = Some(CompletenessCommitSource::UtteranceFinal);
                guard.committed_chars = guard
                    .committed_chars
                    .saturating_add(text.trim().chars().count());
                // Monotonic max: an out-of-order final never rewinds the boundary.
                // Non-finite end_ts must never poison it (parity with
                // ComposerTranscript::note_committed_through): NaN falls through
                // the `current >= end_ts` arm and would overwrite a valid max.
                if end_ts.is_finite() {
                    guard.committed_through_secs = Some(match guard.committed_through_secs {
                        Some(current) if current >= *end_ts => current,
                        _ => *end_ts,
                    });
                }
            }
            EngineEvent::SessionFinalised { .. } => {
                guard.pending_tail = false;
                guard.last_commit_source = Some(CompletenessCommitSource::SessionFinalised);
            }
            // Layer 1 recovered speech it could not place. Set-only: a hole
            // found mid-session stays known until the session is reset, because
            // the missing speech does not come back on its own. The exact code
            // is the whole contract — a near-miss code must leave the flag false
            // rather than put a Whisper pass on every stop path that logs a
            // warning.
            EngineEvent::Warning { code, .. } if code == UNDER_COMMIT_WARNING_CODE => {
                guard.residual_required = true;
            }
            EngineEvent::Stats {
                hallucination_drops,
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

/// Controller-helper coverage, in four groups: tool-label mapping for the
/// conversation timeline, session telemetry folding, agent runtime lifecycle
/// (degrade, rehydrate, identity continuity), and the send path itself
/// (cancellation, persistence, legacy fallback).
///
/// Tests that drive the send path publish to a process-global delivery
/// broadcast, so they serialize on `SEND_PATH_BROADCAST_LOCK`.
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use codescribe_core::agent::{
        AgentEvent, AgentProvider, ContentBlock, Message, Role, ToolDefinition, ToolResultContent,
    };
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    // ── Assistive routing target (operator contract 2026-08-13) ─────────────

    /// Retargeting to another persisted thread drops the runtime (so the next
    /// send rejoins + rehydrates) and adopts the new identity; retargeting to
    /// the SAME thread must not touch a live runtime — steady-state sends may
    /// re-apply the target on every turn.
    #[test]
    fn retarget_thread_rebinds_on_change_and_noops_on_same() {
        let mut state = AgentRuntimeState {
            runtime: None,
            thread_store_id: Some("thread-a".to_string()),
            runtime_degraded: false,
        };

        state.retarget_thread(Some("thread-a".to_string()));
        assert_eq!(state.thread_store_id.as_deref(), Some("thread-a"));

        state.retarget_thread(Some("thread-b".to_string()));
        assert_eq!(
            state.thread_store_id.as_deref(),
            Some("thread-b"),
            "a changed selection must adopt the new identity"
        );
        assert!(
            state.runtime.is_none(),
            "rebind goes through the rejoin machinery (runtime dropped)"
        );
    }

    /// A `None` target is the explicit "+ New thread": the durable identity is
    /// cleared so the next send mints a fresh thread instead of continuing the
    /// previous conversation.
    #[test]
    fn retarget_thread_none_clears_identity_for_a_fresh_mint() {
        let mut state = AgentRuntimeState {
            runtime: None,
            thread_store_id: Some("thread-a".to_string()),
            runtime_degraded: false,
        };
        state.retarget_thread(None);
        assert!(state.thread_store_id.is_none());
    }

    // ── Collapsible Tool Evidence: friendly tool-name mapping ───────────────

    /// Both the MCP wire form and the bare tool id must resolve to the same
    /// human label, so a server rename cannot change what the user reads.
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

    /// The generic fallback must stay presentable for names nobody mapped:
    /// never leak the raw `mcp__` wire form, never emit a dangling separator for
    /// a trailing `__`, and never return an empty label for a degenerate id.
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

    /// The explicit operator label table wins over the generic fallback. These
    /// ids all prettified into reversed or noisy copy before the table existed
    /// (e.g. "Context · Loctree mcp", "Read Clipboard").
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

    /// Runtime-side counterpart to the pure display test: proves the controller
    /// maps a real observed tool sequence to the labels the timeline expects,
    /// rather than the timeline test hardcoding the display names it wants.
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

    /// Provider that never emits anything: its event channel is closed
    /// immediately. Used where a session must exist but must not produce
    /// conversation history of its own.
    struct NoopTestProvider;

    #[async_trait]
    impl AgentProvider for NoopTestProvider {
        /// Return an already-closed receiver — the sender is dropped on return.
        async fn stream(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &StreamOptions,
        ) -> anyhow::Result<mpsc::Receiver<AgentEvent>> {
            let (_tx, rx) = mpsc::channel(1);
            Ok(rx)
        }

        /// Wrap tool output as a user `ToolResult` message for the no-op test provider.
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

        /// Build an image content block from raw bytes and media type.
        fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
            ContentBlock::Image {
                data: data.to_vec(),
                media_type: media_type.to_string(),
            }
        }

        /// Stable provider name shown in tests and diagnostics.
        fn name(&self) -> &str {
            "noop-test-provider"
        }
    }

    /// A runtime bound to an explicit thread id and backed by the no-op
    /// provider, for lifecycle tests that only care about identity handling.
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

    /// Every `Stats` counter must survive the fold field-for-field — the
    /// controller routes on these numbers, so a dropped or transposed field is
    /// a silent behaviour change. `NoSpeech` is captured alongside them.
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
        assert!(!snapshot.pending_tail);
        assert!(snapshot.last_commit_source.is_none());
        let stats = snapshot.stats.expect("stats should be captured");
        assert_eq!(stats.hallucination_drops, 2);
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

    /// The open-tail state machine: Preview and Correction open a tail, and only
    /// a commit closes it — recording which commit did so. A Correction arriving
    /// after a final re-opens the tail, because there is again uncommitted text.
    #[test]
    fn test_session_telemetry_tracks_pending_tail_and_commit_source() {
        let shared = new_session_telemetry();
        let sink = SessionTelemetrySink::new(Arc::clone(&shared));

        sink.on_event(&EngineEvent::Preview {
            rev: 1,
            text: "To jest".to_string(),
        });
        let open = snapshot_session_telemetry(&shared);
        assert!(open.pending_tail, "preview leaves a pending tail");
        assert!(open.last_commit_source.is_none());

        sink.on_event(&EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "To jest kompletne zdanie.".to_string(),
            raw_text: "To jest kompletne zdanie.".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: vec![],
            vad_speech_pct: Some(80.0),
            avg_logprob: None,
            compression_ratio: None,
            confidence_flags: vec![],
        });
        let sealed = snapshot_session_telemetry(&shared);
        assert!(!sealed.pending_tail);
        assert_eq!(
            sealed.last_commit_source,
            Some(CompletenessCommitSource::UtteranceFinal)
        );
        assert_eq!(
            sealed.committed_chars,
            "To jest kompletne zdanie.".chars().count()
        );

        sink.on_event(&EngineEvent::Correction {
            rev: 2,
            text: "poprawka".to_string(),
            previous_text: "To jest kompletne zdanie.".to_string(),
        });
        assert!(snapshot_session_telemetry(&shared).pending_tail);

        sink.on_event(&EngineEvent::SessionFinalised {
            session_id: "s1".to_string(),
            layer_summary: Default::default(),
        });
        let finalised = snapshot_session_telemetry(&shared);
        assert!(!finalised.pending_tail);
        assert_eq!(
            finalised.last_commit_source,
            Some(CompletenessCommitSource::SessionFinalised)
        );
    }

    /// Smart-mode tail transcription needs the audio boundary of committed
    /// streaming text: the monotonic max `end_ts` across UtteranceFinal events.
    /// Out-of-order finals must never pull the boundary backwards.
    #[test]
    fn test_session_telemetry_tracks_committed_through_secs_monotonic_max() {
        let shared = new_session_telemetry();
        let sink = SessionTelemetrySink::new(Arc::clone(&shared));

        assert!(
            snapshot_session_telemetry(&shared)
                .committed_through_secs
                .is_none(),
            "default snapshot has no committed audio boundary"
        );

        let utterance_final =
            |utterance_id: u64, start_ts: f32, end_ts: f32| EngineEvent::UtteranceFinal {
                utterance_id,
                text: "zdanie".to_string(),
                raw_text: "zdanie".to_string(),
                start_ts,
                end_ts,
                segments: vec![],
                vad_speech_pct: Some(80.0),
                avg_logprob: None,
                compression_ratio: None,
                confidence_flags: vec![],
            };

        sink.on_event(&utterance_final(1, 0.0, 3.2));
        assert_eq!(
            snapshot_session_telemetry(&shared).committed_through_secs,
            Some(3.2)
        );

        sink.on_event(&utterance_final(2, 3.2, 7.9));
        assert_eq!(
            snapshot_session_telemetry(&shared).committed_through_secs,
            Some(7.9)
        );

        // Out-of-order final: boundary stays at the max already committed.
        sink.on_event(&utterance_final(3, 4.0, 5.0));
        assert_eq!(
            snapshot_session_telemetry(&shared).committed_through_secs,
            Some(7.9),
            "committed boundary is a monotonic max, not the last value"
        );

        reset_session_telemetry(&shared);
        assert!(
            snapshot_session_telemetry(&shared)
                .committed_through_secs
                .is_none(),
            "reset clears the committed audio boundary"
        );
    }

    /// Parity with `ComposerTranscript::note_committed_through` (PR #69
    /// review): a non-finite `end_ts` (NaN/±inf) must never poison or advance
    /// the boundary. NaN falls through the `current >= end_ts` guard and would
    /// otherwise OVERWRITE a valid max — silently degrading Smart tail
    /// gap-fill to Skip for the rest of the session.
    #[test]
    fn test_session_telemetry_ignores_non_finite_end_ts() {
        let shared = new_session_telemetry();
        let sink = SessionTelemetrySink::new(Arc::clone(&shared));

        let utterance_final =
            |utterance_id: u64, start_ts: f32, end_ts: f32| EngineEvent::UtteranceFinal {
                utterance_id,
                text: "zdanie".to_string(),
                raw_text: "zdanie".to_string(),
                start_ts,
                end_ts,
                segments: vec![],
                vad_speech_pct: Some(80.0),
                avg_logprob: None,
                compression_ratio: None,
                confidence_flags: vec![],
            };

        sink.on_event(&utterance_final(1, 0.0, 3.2));
        sink.on_event(&utterance_final(2, 3.2, f32::NAN));
        assert_eq!(
            snapshot_session_telemetry(&shared).committed_through_secs,
            Some(3.2),
            "NaN end_ts must not overwrite the committed boundary"
        );

        sink.on_event(&utterance_final(3, 3.2, f32::INFINITY));
        assert_eq!(
            snapshot_session_telemetry(&shared).committed_through_secs,
            Some(3.2),
            "+inf end_ts must not advance the boundary"
        );

        sink.on_event(&utterance_final(4, 3.2, f32::NEG_INFINITY));
        assert_eq!(
            snapshot_session_telemetry(&shared).committed_through_secs,
            Some(3.2),
            "-inf end_ts must not disturb the boundary"
        );

        sink.on_event(&utterance_final(5, 3.2, 4.5));
        assert_eq!(
            snapshot_session_telemetry(&shared).committed_through_secs,
            Some(4.5),
            "finite finals keep advancing after non-finite noise"
        );
    }

    /// The stop path's residual demand is folded by warning CODE, not by
    /// variant. Three things are pinned here because each is a different way to
    /// break the contract: exactly `tail_patch_under_commit` sets the flag, its
    /// neighbours must not (a loose match would put a Whisper pass on the stop
    /// path of every session that logs a warning), and once set no later event
    /// may clear it — the speech Layer 1 could not place does not come back on
    /// its own, so a clean final afterwards is not evidence the hole closed.
    #[test]
    fn test_session_telemetry_folds_only_the_exact_under_commit_warning() {
        let warning = |code: &str| EngineEvent::Warning {
            code: code.to_string(),
            message: "committed_tokens=3 retranscribed_tokens=12".to_string(),
        };
        let utterance_final = || EngineEvent::UtteranceFinal {
            utterance_id: 1,
            text: "zdanie".to_string(),
            raw_text: "zdanie".to_string(),
            start_ts: 0.0,
            end_ts: 1.0,
            segments: vec![],
            vad_speech_pct: Some(80.0),
            avg_logprob: None,
            compression_ratio: None,
            confidence_flags: vec![],
        };

        // Near misses: the sibling receipt code, a truncation, an extension, a
        // case variant, a bare substring, and the empty code.
        for code in [
            "tail_patch_skipped",
            "tail_patch_under_commi",
            "tail_patch_under_commit_residual",
            "TAIL_PATCH_UNDER_COMMIT",
            "under_commit",
            "",
        ] {
            let shared = new_session_telemetry();
            let sink = SessionTelemetrySink::new(Arc::clone(&shared));
            sink.on_event(&warning(code));
            assert!(
                !snapshot_session_telemetry(&shared).residual_required,
                "warning code {code:?} must not demand residual gap fill"
            );
        }

        let shared = new_session_telemetry();
        let sink = SessionTelemetrySink::new(Arc::clone(&shared));
        assert!(
            !snapshot_session_telemetry(&shared).residual_required,
            "a fresh session starts with no residual demand"
        );

        sink.on_event(&warning(UNDER_COMMIT_WARNING_CODE));
        assert!(
            snapshot_session_telemetry(&shared).residual_required,
            "the exact Layer 1 under-commit code must fold to residual_required"
        );

        // Monotonic within the session: a clean commit, an unrelated warning
        // and the session seal all arrive after the escalation and none of them
        // may un-know it.
        sink.on_event(&utterance_final());
        sink.on_event(&warning("tail_patch_skipped"));
        sink.on_event(&EngineEvent::SessionFinalised {
            session_id: "s1".to_string(),
            layer_summary: Default::default(),
        });
        assert!(
            snapshot_session_telemetry(&shared).residual_required,
            "later healthy events must not clear a hole Layer 1 already found"
        );

        // Only a new session clears it.
        reset_session_telemetry(&shared);
        assert!(
            !snapshot_session_telemetry(&shared).residual_required,
            "reset clears the residual demand so a new recording never inherits it"
        );
    }

    /// Reset must clear every field, not just the obvious ones: leftover
    /// `pending_tail` or `committed_chars` would make the next session's first
    /// routing decision read the previous session's state.
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
            guard.pending_tail = true;
            guard.last_commit_source = Some(CompletenessCommitSource::UtteranceFinal);
            guard.committed_chars = 12;
            guard.committed_through_secs = Some(41.0);
            guard.residual_required = true;
        }
        reset_session_telemetry(&shared);

        let snapshot = snapshot_session_telemetry(&shared);
        assert!(snapshot.no_speech_reason.is_none());
        assert!(snapshot.stats.is_none());
        assert!(!snapshot.pending_tail);
        assert!(snapshot.last_commit_source.is_none());
        assert_eq!(snapshot.committed_chars, 0);
        assert!(snapshot.committed_through_secs.is_none());
        assert!(
            !snapshot.residual_required,
            "a stale residual demand would force a Whisper tail pass on the next session"
        );
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

    /// Rebuilding after a degrade must rejoin the durable thread id and discard
    /// the id the fresh runtime minted — and it must clear the degraded flag,
    /// reporting `recovered = true` so the UI can retire the banner.
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

    /// A mid-stream provider failure already owns and publishes the terminal UI
    /// event, so the controller must not publish a duplicate terminal.
    #[test]
    fn test_provider_stream_errors_are_already_published() {
        let error = anyhow::anyhow!(
            "Provider stream error: Agent SSE error internal_error: 'list' object has no attribute 'uid'"
        );

        assert!(agent_send_error_was_published(&error));
    }

    /// Provider that completes one clean turn so the seeded session ends up with
    /// both conversation history AND a provider thread id (chain) set.
    struct CompletingTestProvider;

    #[async_trait]
    impl AgentProvider for CompletingTestProvider {
        /// Emit a finished turn: one assistant message plus a clean
        /// `ResponseDone` carrying a response id, so the session ends with both
        /// history and a provider chain to later assert against.
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

        /// Wrap tool output as a user `ToolResult` message for the completing test provider.
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

        /// Build an image content block from raw bytes and media type.
        fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
            ContentBlock::Image {
                data: data.to_vec(),
                media_type: media_type.to_string(),
            }
        }

        /// Stable provider name shown in tests and diagnostics.
        fn name(&self) -> &str {
            "completing-test-provider"
        }
    }

    /// A runtime that has already completed one turn, so degrade tests start
    /// from real state: non-empty history AND a set provider chain. Drives the
    /// seed turn on its own current-thread runtime so the helper stays callable
    /// from sync `#[test]` fns.
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

    /// The in-flight marker is a count, not a flag: an inner guard dropping
    /// must not clear the state while the outer send is still running.
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

    /// A runtime that never reached the provider still needs one explicit
    /// terminal UI event from the controller boundary.
    #[test]
    fn test_runtime_unavailable_errors_need_terminal_publication() {
        let error = anyhow::anyhow!("Agent runtime unavailable");

        assert!(!agent_send_error_was_published(&error));
    }

    /// Text with no attachment marker must come through byte-identical — the
    /// splitter must not rewrite ordinary messages on its way past them.
    #[test]
    fn test_build_image_attachments_passthrough_without_marker() {
        let text = "plain message, no attachments";
        let (cleaned, images, dropped) = build_image_attachments_from_text(text);
        assert_eq!(cleaned, text);
        assert!(images.is_empty());
        assert!(dropped.is_empty());
    }

    /// The marker block and its raw paths must disappear from model-visible
    /// text while the readable image becomes real vision input. An unreadable
    /// path is reported as dropped, never silently forwarded as text — that
    /// silent passthrough was the original attachment bug.
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

    /// Past the cap, the overflow is surfaced rather than quietly discarded:
    /// exactly `MAX_AGENT_VISION_IMAGES` are forwarded and the remainder come
    /// back as dropped names the caller can show.
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

    /// Every UI event variant must cross to its delivery twin unchanged. The two
    /// enums are kept the same shape on purpose, so the Swift listener stays
    /// symmetric with the composer's — a mismapped arm would desync them.
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

    /// Draining an event must also publish it: this is the only path by which
    /// the SwiftUI chat learns anything. Matched on a unique payload because the
    /// broadcast is process-global and other tests publish to it concurrently.
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

    /// Provider driven by a queue of pre-written turns, one script popped per
    /// call. Records what it was asked to do as well as what it answered, so
    /// continuity tests can assert on the inputs the runtime actually replayed
    /// and on the `reset_chain` flag each turn carried.
    struct ScriptedControllerProvider {
        scripts: Arc<StdMutex<VecDeque<Vec<AgentEvent>>>>,
        reset_chain_flags: Arc<StdMutex<Vec<bool>>>,
        /// Full provider-call inputs, recorded so continuity tests can prove the
        /// second call replays prior history instead of just the new message.
        seen_inputs: Arc<StdMutex<Vec<Vec<Message>>>>,
    }

    #[async_trait]
    impl AgentProvider for ScriptedControllerProvider {
        /// Record this call's inputs and `reset_chain` flag, then replay the
        /// next queued script from a spawned task so the receiver is returned
        /// before the events land — mirroring a real streaming provider rather
        /// than handing back an already-full channel.
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

        /// Wrap tool output as a user `ToolResult` message for the scripted test provider.
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

        /// Build an image content block from raw bytes and media type.
        fn build_image_block(&self, data: &[u8], media_type: &str) -> ContentBlock {
            ContentBlock::Image {
                data: data.to_vec(),
                media_type: media_type.to_string(),
            }
        }

        /// Stable provider name shown in tests and diagnostics.
        fn name(&self) -> &str {
            "scripted-controller-provider"
        }
    }

    /// Yield until a flag is set, with a hard timeout so a wedged test fails
    /// loudly instead of hanging the suite.
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

    /// The full user-Stop contract in one turn. Cancelling mid-tool must: drop
    /// the in-flight tool future so its side effect never fires, restore the
    /// pre-turn history, skip persistence, emit exactly one keyed `Cancelled`
    /// terminal and no `Done`/`Error`, and clean its registry token. Then the
    /// next turn must succeed on the same runtime while keeping the pre-turn
    /// chain — Stop cancels a turn, it does not reset the conversation.
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
            .register_native(
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
                // Classified read-only so the permission gate stays out of the
                // way: this test is about cancel dropping an in-flight tool
                // future, and the voice lane has no approval broker (see
                // `agent_ui_event_to_delivery`). The "side effect" is the
                // handler's own observable flag, not a risk class.
                codescribe_core::agent::ToolRisk::ReadOnly,
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
                unexpected_runtime_initialization,
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
        assert!(
            !cancelled_runtime.reset_chain_on_next_send,
            "user Stop must preserve the pre-turn response chain for the next send"
        );
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
            unexpected_runtime_initialization,
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
            vec![false, false],
            "the recovery turn after user Stop keeps the pre-turn chain (no forced full-reset)"
        );
    }

    /// Neutral stream options: no model, no prompt, chain preserved — so send
    /// path tests observe the path's own behaviour, not an option's.
    fn test_stream_options() -> StreamOptions {
        StreamOptions {
            model: String::new(),
            system_prompt: None,
            max_tokens: None,
            temperature: None,
            reset_chain: false,
        }
    }

    fn unexpected_runtime_initialization() -> Result<AgentRuntime> {
        Err(anyhow::anyhow!(
            "test expected the installed Agent runtime to remain authoritative"
        ))
    }

    // ── Voice runtime identity and history continuity (W1-A) ────────────────

    /// A runtime on the scripted provider, bound to an explicit thread id and
    /// sharing the caller's `seen_inputs` recorder so continuity assertions can
    /// inspect what the provider was replayed.
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

    /// One clean finished turn for the script queue: an assistant answer plus a
    /// `ResponseDone` that advances the chain to `response_id`.
    fn completed_turn_script(assistant_text: &str, response_id: &str) -> Vec<AgentEvent> {
        vec![
            AgentEvent::TextDone(assistant_text.to_string()),
            AgentEvent::ResponseDone {
                response_id: Some(response_id.to_string()),
                clean: true,
            },
        ]
    }

    /// Assert one voice conversation left exactly one thread file and one index
    /// row. This is the on-disk proof of identity continuity: a runtime that
    /// rotated its thread id would pass in-memory checks but leave two
    /// artifacts here.
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
                unexpected_runtime_initialization,
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
            unexpected_runtime_initialization,
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
            unexpected_runtime_initialization,
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
        /// Tracing writer that appends into a shared buffer, so the test can
        /// read back what the lifecycle actually logged.
        struct SharedWriter(Arc<StdMutex<Vec<u8>>>);
        impl std::io::Write for SharedWriter {
            /// Append to the shared buffer, recovering from a poisoned lock —
            /// a panicking test must not also lose its captured evidence.
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .extend_from_slice(data);
                Ok(data.len())
            }
            /// No-op: the buffer is in memory, so there is nothing to flush.
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
