//! Voice-assistive agent delivery — forwards the app-side delivery broadcast
//! (`codescribe::agent_delivery`) onto a Swift `CsAgentDeliveryListener`.
//!
//! This is the bridge half of the fix for the voice-assistive delivery gap: the
//! hotkey/voice reply used to die in `apply_agent_ui_event` (only persisted to
//! disk, never rendered live) after the legacy AppKit overlay sink was removed.
//! The app now publishes each reply event to a process-global broadcast; this
//! module subscribes and hops them across the UniFFI boundary to SwiftUI.
//!
//! It is the exact mirror of the transcription forwarder in `hotkeys.rs`
//! (`spawn_event_forwarder` / `forward_event_to_listener`), including the
//! `Lagged` (recoverable — keep forwarding) and `Closed` (sender dropped — end
//! the task) handling. Kept a separate listener + channel from
//! `CsTranscriptionListener` so agent-chat delivery never mixes with the
//! overlay/dictation stream.

use std::sync::{Arc, OnceLock, RwLock};

use codescribe::agent_delivery::{AgentDeliveryEvent, subscribe_agent_delivery};
use tokio::runtime::Handle;
use tokio::sync::broadcast::error::RecvError;

/// Foreign callback trait — voice-assistive agent delivery events forwarded to
/// Swift. Symmetric to `CsAgentListener` (the composer path) plus an
/// `on_turn_started` opener carrying the correlation id + the user's transcript.
/// The Swift side must hop each callback onto the main actor (see
/// `VoiceDeliveryListener`).
#[uniffi::export(with_foreign)]
pub trait CsAgentDeliveryListener: Send + Sync {
    /// A new voice turn began: open a You-bubble (`user_text`) + an assistant
    /// placeholder. `thread_id` is the core runtime persistence id, used to
    /// correlate the turn to a store thread.
    fn on_turn_started(&self, thread_id: String, user_text: String);
    fn on_text_delta(&self, delta: String);
    fn on_text_done(&self, text: String);
    fn on_reasoning_delta(&self, delta: String);
    fn on_tool_executing(&self, name: String, id: String);
    fn on_tool_result(&self, name: String, id: String, summary: String, is_error: bool);
    fn on_done(&self);
    fn on_error(&self, message: String);
}

type SharedDeliveryListener = Arc<RwLock<Option<Arc<dyn CsAgentDeliveryListener>>>>;

fn shared_delivery_listener() -> SharedDeliveryListener {
    static LISTENER: OnceLock<SharedDeliveryListener> = OnceLock::new();
    Arc::clone(LISTENER.get_or_init(|| Arc::new(RwLock::new(None))))
}

/// Register the Swift AgentChat delivery listener. Process-global (mirrors
/// `hotkeys::shared_listener`) so registration is independent of which
/// `CodescribeHotkeys` handle the forwarder was spawned from.
pub(crate) fn set_delivery_listener(listener: Arc<dyn CsAgentDeliveryListener>) {
    let store = shared_delivery_listener();
    let mut guard = store.write().unwrap_or_else(|e| e.into_inner());
    *guard = Some(listener);
}

/// Spawn the app→bridge delivery forwarder on `handle`.
///
/// Idempotent: guarded by a `OnceLock` so repeated `start()` calls (or a
/// listener re-registration) never stack duplicate forwarders that would each
/// deliver the same event.
pub(crate) fn spawn_delivery_forwarder(handle: Handle) {
    static SPAWNED: OnceLock<()> = OnceLock::new();
    if SPAWNED.set(()).is_err() {
        return;
    }
    let listener_store = shared_delivery_listener();
    let mut events = subscribe_agent_delivery();
    handle.spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                // Lagged: a burst of token deltas overflowed the broadcast (cap
                // 256) and dropped `skipped` events. Recoverable — keep
                // forwarding subsequent events instead of tearing the bridge down.
                Err(RecvError::Lagged(skipped)) => {
                    eprintln!("Agent delivery forwarder lagged; dropped {skipped} event(s)");
                    continue;
                }
                // Closed: the app-side sender was dropped — nothing more arrives.
                Err(RecvError::Closed) => break,
            };
            let listener = listener_store
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map(Arc::clone);
            let Some(listener) = listener else {
                // No Swift listener registered yet (or chat surface not built):
                // drop the event, the reply is still persisted to disk app-side.
                continue;
            };
            forward_delivery_event(event, listener);
        }
    });
}

fn forward_delivery_event(event: AgentDeliveryEvent, listener: Arc<dyn CsAgentDeliveryListener>) {
    match event {
        AgentDeliveryEvent::TurnStarted {
            thread_id,
            user_text,
        } => listener.on_turn_started(thread_id, user_text),
        AgentDeliveryEvent::TextDelta(delta) => listener.on_text_delta(delta),
        AgentDeliveryEvent::TextDone(text) => listener.on_text_done(text),
        AgentDeliveryEvent::ReasoningDelta(delta) => listener.on_reasoning_delta(delta),
        AgentDeliveryEvent::ToolExecuting { name, id } => listener.on_tool_executing(name, id),
        AgentDeliveryEvent::ToolResult {
            name,
            id,
            summary,
            is_error,
        } => listener.on_tool_result(name, id, summary, is_error),
        AgentDeliveryEvent::Done => listener.on_done(),
        AgentDeliveryEvent::Error(message) => listener.on_error(message),
    }
}
