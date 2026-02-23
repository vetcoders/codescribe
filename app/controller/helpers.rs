//! Controller helper functions
//!
//! Session state management and utility functions.

use chrono::Utc;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex as TokioMutex, RwLock, mpsc};
use tracing::{debug, warn};

use crate::config::Config;
use anyhow::{Context, Result};
use codescribe_core::agent::{
    AgentSession, AgentUiEvent, ContentBlock, Message, Role, StreamOptions, Thread, ThreadMessage,
    ThreadStore, ToolRegistry,
};

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
/// - Non-assistive sessions stream into Dictation/Transcription overlay preview.
pub fn route_transcription_delta(delta: &str) {
    if is_assistive_session() {
        crate::voice_chat_ui::append_voice_chat_user_delta(delta);
    } else {
        // Non-assistive: live dictation preview in ephemeral overlay
        crate::transcription_overlay::append_transcription_delta(delta);
    }
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

struct AgentRuntime {
    session: AgentSession,
    ui_rx: mpsc::Receiver<AgentUiEvent>,
    thread_store_id: String,
}

#[derive(Default)]
struct AgentRuntimeState {
    runtime: Option<AgentRuntime>,
}

#[derive(Default)]
struct AgentUiOverlayState {
    streamed_any_delta: bool,
    saw_reasoning_delta: bool,
}

impl AgentRuntimeState {
    fn ensure_runtime(&mut self) -> Result<&mut AgentRuntime> {
        if self.runtime.is_none() {
            self.runtime = Some(initialize_agent_runtime()?);
        }
        self.runtime
            .as_mut()
            .context("Agent runtime was not initialized")
    }

    fn invalidate_runtime(&mut self) {
        self.runtime = None;
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

    StreamOptions {
        model: String::new(),
        system_prompt: Some(crate::config::get_assistive_prompt()),
        max_tokens,
        temperature: None,
    }
}

fn apply_agent_ui_event(event: AgentUiEvent, overlay_state: &mut AgentUiOverlayState) {
    match event {
        AgentUiEvent::TextDelta(delta) => {
            if delta.is_empty() {
                return;
            }
            if !overlay_state.streamed_any_delta && !delta.trim().is_empty() {
                crate::voice_chat_ui::update_voice_chat_status("Answering... (80%)");
            }
            overlay_state.streamed_any_delta = true;
            crate::voice_chat_ui::append_voice_chat_assistant_delta(&delta);
        }
        AgentUiEvent::TextDone(text) => {
            if !overlay_state.streamed_any_delta && !text.trim().is_empty() {
                crate::voice_chat_ui::set_voice_chat_text(&text);
            }
        }
        AgentUiEvent::ReasoningDelta(delta) => {
            if delta.trim().is_empty() {
                return;
            }
            if !overlay_state.saw_reasoning_delta {
                crate::voice_chat_ui::update_voice_chat_status("Reasoning... (60%)");
                overlay_state.saw_reasoning_delta = true;
            }
        }
        AgentUiEvent::ToolExecuting { name, .. } => {
            crate::voice_chat_ui::update_voice_chat_status(&format!("Tool running: {name}"));
            crate::voice_chat_ui::add_voice_chat_system_message(&format!(
                "Tool call started: {name}"
            ));
        }
        AgentUiEvent::ToolResult { name, summary, .. } => {
            crate::voice_chat_ui::update_voice_chat_status("Thinking... (70%)");
            crate::voice_chat_ui::add_voice_chat_system_message(&format!(
                "Tool call finished: {name} ({summary})"
            ));
        }
        AgentUiEvent::Done => {}
        AgentUiEvent::Error(message) => {
            crate::voice_chat_ui::update_voice_chat_status("Agent runtime failed");
            crate::voice_chat_ui::add_voice_chat_error_message(&message);
        }
    }
}

fn extract_text_from_block(block: &ContentBlock, out: &mut Vec<String>) {
    match block {
        ContentBlock::Text(text) => {
            if !text.trim().is_empty() {
                out.push(text.to_string());
            }
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
        .unwrap_or_else(|| "CodeScribe Agent Chat".to_string());

    let mut title = candidate.chars().take(72).collect::<String>();
    if title.is_empty() {
        title = "CodeScribe Agent Chat".to_string();
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
            title: "CodeScribe Agent Chat".to_string(),
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
    thread.title = derive_thread_title(runtime.session.messages());
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

async fn run_agent_send_path(
    runtime_state: &mut AgentRuntimeState,
    text: String,
    stream_options: StreamOptions,
) -> Result<()> {
    let runtime = runtime_state.ensure_runtime()?;
    let mut overlay_state = AgentUiOverlayState::default();

    let send_result = {
        let (session, ui_rx) = (&mut runtime.session, &mut runtime.ui_rx);
        let send_future = session.send(text, Vec::new(), &stream_options);
        tokio::pin!(send_future);

        let result = loop {
            tokio::select! {
                result = &mut send_future => break result,
                maybe_event = ui_rx.recv() => {
                    match maybe_event {
                        Some(event) => apply_agent_ui_event(event, &mut overlay_state),
                        None => break Err(anyhow::anyhow!("Agent UI event channel closed")),
                    }
                }
            }
        };

        while let Ok(event) = ui_rx.try_recv() {
            apply_agent_ui_event(event, &mut overlay_state);
        }

        result
    };

    match send_result {
        Ok(()) => {
            crate::voice_chat_ui::update_voice_chat_status("AI Response:");
            if overlay_state.streamed_any_delta {
                crate::voice_chat_ui::finalize_voice_chat_assistant_message();
            }
            if let Err(error) = persist_runtime_thread(runtime) {
                warn!("Failed to persist agent thread: {}", error);
            } else {
                crate::voice_chat_ui::refresh_drawer();
            }
            crate::voice_chat_ui::set_voice_chat_sending(false);
            Ok(())
        }
        Err(error) => {
            if overlay_state.streamed_any_delta {
                crate::voice_chat_ui::finalize_voice_chat_assistant_message();
            }
            runtime_state.invalidate_runtime();
            Err(error).context("AgentSession send failed")
        }
    }
}

async fn run_legacy_send_path(text: &str, whisper_language: crate::config::Language) {
    let use_streaming = true;
    let streamed_any_delta = Arc::new(AtomicBool::new(false));

    let delta_callback = if use_streaming {
        let streamed_any_delta = Arc::clone(&streamed_any_delta);
        Some(Arc::new(move |delta: &str| {
            streamed_any_delta.store(true, Ordering::SeqCst);
            crate::voice_chat_ui::append_voice_chat_assistant_delta(delta);
        }) as Arc<dyn Fn(&str) + Send + Sync>)
    } else {
        None
    };

    let result = crate::ai_formatting::format_text_with_status_channels(
        text,
        Some(whisper_language.as_str()),
        true,
        delta_callback,
        None,
    )
    .await;

    match result.status {
        crate::ai_formatting::AiFormatStatus::Applied => {
            crate::voice_chat_ui::update_voice_chat_status("AI Response:");
            if use_streaming && streamed_any_delta.load(Ordering::SeqCst) {
                crate::voice_chat_ui::finalize_voice_chat_assistant_message();
            } else {
                crate::voice_chat_ui::set_voice_chat_text(&result.text);
            }
            if let Some(reasoning_text) = result.reasoning_text {
                crate::voice_chat_ui::add_voice_chat_system_message(&format!(
                    "Reasoning summary:\n{}",
                    reasoning_text
                ));
            }
        }
        crate::ai_formatting::AiFormatStatus::Failed => {
            crate::voice_chat_ui::update_voice_chat_status("AI Failed");
            crate::voice_chat_ui::add_voice_chat_error_message("AI Failed");
        }
        crate::ai_formatting::AiFormatStatus::Skipped => {
            crate::voice_chat_ui::set_voice_chat_sending(false);
        }
    }
}

/// Setup the voice chat send callback with config
pub fn setup_voice_chat_send_callback(config: Arc<RwLock<Config>>) {
    let initial_runtime_state = match initialize_agent_runtime() {
        Ok(runtime) => AgentRuntimeState {
            runtime: Some(runtime),
        },
        Err(error) => {
            warn!(
                "Agent runtime init failed during callback setup; legacy fallback will be used until retry succeeds: {}",
                error
            );
            AgentRuntimeState::default()
        }
    };
    let runtime_state = Arc::new(TokioMutex::new(initial_runtime_state));

    let callback_config = Arc::clone(&config);
    let callback_runtime_state = Arc::clone(&runtime_state);
    crate::voice_chat_ui::set_voice_chat_send_callback(Some(Arc::new(move |text: String| {
        let config = Arc::clone(&callback_config);
        let runtime_state = Arc::clone(&callback_runtime_state);
        tokio::spawn(async move {
            crate::voice_chat_ui::update_voice_chat_status("Sending...");
            crate::voice_chat_ui::set_voice_chat_sending(true);

            let (whisper_language, ai_assistive_max_tokens) = {
                let cfg = config.read().await;
                (cfg.whisper_language, cfg.ai_assistive_max_tokens)
            };
            let stream_options = build_agent_stream_options(ai_assistive_max_tokens);

            let agent_result = {
                let mut guard = runtime_state.lock().await;
                run_agent_send_path(&mut guard, text.clone(), stream_options).await
            };

            if let Err(error) = agent_result {
                warn!(
                    "Agent runtime failed, switching this response to legacy fallback: {}",
                    error
                );
                debug!("Legacy fallback input length: {}", text.len());
                crate::voice_chat_ui::set_voice_chat_sending(true);
                crate::voice_chat_ui::update_voice_chat_status("Agent fallback active");
                crate::voice_chat_ui::add_voice_chat_system_message(
                    "Agent runtime unavailable. Using legacy formatter for this response.",
                );
                run_legacy_send_path(&text, whisper_language).await;
            }
        });
    })));
}

/// Raw transcript saving is always enabled to avoid data loss.
pub fn raw_save_enabled() -> bool {
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
    pub trigger_watchdog_count: u64,
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
                trigger_watchdog_count,
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
                    trigger_watchdog_count: *trigger_watchdog_count,
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
            trigger_watchdog_count: 1,
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
        assert_eq!(stats.trigger_watchdog_count, 1);
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
}
