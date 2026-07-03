//! Voice-assistive agent delivery broadcast.
//!
//! The hotkey / voice-assistive send path streams the agent reply as
//! [`codescribe_core::agent::AgentUiEvent`]s inside
//! [`crate::controller::helpers`]'s `apply_agent_ui_event`. That drain runs in
//! the `codescribe` app crate, which sits *below* the `codescribe-ffi` bridge in
//! the dependency graph (`bridge -> codescribe -> codescribe-core`). It therefore
//! cannot call the Swift-facing UniFFI listener directly.
//!
//! This module owns a process-global `tokio::sync::broadcast` channel that the
//! app publishes turn / delta / tool / done events into; the bridge subscribes
//! and forwards each event onto a `CsAgentDeliveryListener`. It is the exact
//! mirror of the transcription forwarder (`bridge::hotkeys::spawn_event_forwarder`
//! over `RecordingController::subscribe_events`) — same broadcast + `Lagged`/
//! `Closed` handling — but semantically dedicated to AgentChat so it never mixes
//! with the overlay/dictation `CsTranscriptionListener` stream.
//!
//! Legacy AppKit overlay delivery removed the sink that used to render these
//! events; this broadcast is its replacement. Persistence to disk still happens
//! in `apply`'s caller (`persist_runtime_thread`); this channel only carries the
//! *live* render.

use std::sync::OnceLock;

use tokio::sync::broadcast;

/// Broadcast channel capacity. Sized like the transcription IPC channel (256): a
/// burst of token deltas during a fast reply must not lag a slow consumer into a
/// permanent tear-down. `Lagged` is handled gracefully on the bridge forwarder.
const AGENT_DELIVERY_CHANNEL_CAPACITY: usize = 256;

/// One voice-assistive agent delivery event. Provider-agnostic; mirrors the
/// subset of `AgentUiEvent` the chat UI renders, plus a [`TurnStarted`] opener
/// that carries the correlation `thread_id` and the user's transcript so the UI
/// can open a fresh You-bubble + assistant placeholder before the reply streams.
///
/// [`TurnStarted`]: AgentDeliveryEvent::TurnStarted
#[derive(Debug, Clone, PartialEq)]
pub enum AgentDeliveryEvent {
    /// A new voice turn began. `thread_id` is the core runtime's
    /// `thread_store_id` (the persistence id, disjoint from the SwiftUI store's
    /// per-thread `UUID`); `user_text` is the finalized transcript sent to the
    /// agent.
    TurnStarted {
        thread_id: String,
        user_text: String,
    },
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
        is_error: bool,
    },
    Done,
    Error(String),
}

static AGENT_DELIVERY_TX: OnceLock<broadcast::Sender<AgentDeliveryEvent>> = OnceLock::new();

fn sender() -> &'static broadcast::Sender<AgentDeliveryEvent> {
    AGENT_DELIVERY_TX.get_or_init(|| broadcast::channel(AGENT_DELIVERY_CHANNEL_CAPACITY).0)
}

/// Publish a delivery event to all subscribers. Lock-free and non-blocking.
///
/// A send with no live subscribers returns `Err` (the event is dropped) — that
/// is expected and intentionally ignored: the reply is still persisted to disk
/// by the send path regardless of whether a UI is currently listening.
pub fn publish_agent_delivery_event(event: AgentDeliveryEvent) {
    let _ = sender().send(event);
}

/// Subscribe to the voice-assistive agent delivery stream. The bridge calls this
/// once at startup and spawns a forwarder task that translates each event onto
/// the registered `CsAgentDeliveryListener`.
pub fn subscribe_agent_delivery() -> broadcast::Receiver<AgentDeliveryEvent> {
    sender().subscribe()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::RecvError;

    /// Receive on `rx` until `matches(event)` holds, skipping any interleaved
    /// events from other tests that share this process-global channel. Bounded so
    /// a missing event fails the test instead of hanging.
    async fn recv_until(
        rx: &mut broadcast::Receiver<AgentDeliveryEvent>,
        mut matches: impl FnMut(&AgentDeliveryEvent) -> bool,
    ) -> AgentDeliveryEvent {
        for _ in 0..1024 {
            match rx.recv().await {
                Ok(event) if matches(&event) => return event,
                Ok(_) => continue,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => panic!("delivery channel closed unexpectedly"),
            }
        }
        panic!("expected event never arrived on the delivery channel");
    }

    #[tokio::test]
    async fn published_turn_started_reaches_a_subscriber() {
        // Unique thread id so a concurrent test on the shared global channel can
        // never satisfy this test's matcher.
        let thread_id = "t_turn_started_reaches_subscriber".to_string();
        let mut rx = subscribe_agent_delivery();
        publish_agent_delivery_event(AgentDeliveryEvent::TurnStarted {
            thread_id: thread_id.clone(),
            user_text: "hello".to_string(),
        });
        let received = recv_until(&mut rx, |event| {
            matches!(event, AgentDeliveryEvent::TurnStarted { thread_id: t, .. } if *t == thread_id)
        })
        .await;
        assert_eq!(
            received,
            AgentDeliveryEvent::TurnStarted {
                thread_id,
                user_text: "hello".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn delta_then_done_preserve_sender_order() {
        // A single sender guarantees per-channel FIFO for its own events even
        // when other tests interleave; assert order across a uniquely-tagged
        // pair by filtering to just this test's markers.
        let tag = "delta_then_done_preserve_sender_order";
        let delta = format!("{tag}:delta");
        let mut rx = subscribe_agent_delivery();
        publish_agent_delivery_event(AgentDeliveryEvent::TextDelta(delta.clone()));
        publish_agent_delivery_event(AgentDeliveryEvent::Error(tag.to_string()));

        let first = recv_until(&mut rx, |event| {
            matches!(event, AgentDeliveryEvent::TextDelta(s) if *s == delta)
                || matches!(event, AgentDeliveryEvent::Error(s) if s == tag)
        })
        .await;
        assert_eq!(first, AgentDeliveryEvent::TextDelta(delta));
        let second = recv_until(
            &mut rx,
            |event| matches!(event, AgentDeliveryEvent::Error(s) if s == tag),
        )
        .await;
        assert_eq!(second, AgentDeliveryEvent::Error(tag.to_string()));
    }

    #[test]
    fn publish_without_subscriber_is_silent() {
        // No subscriber attached: the send returns Err internally but the public
        // API must not panic or block (the disk persist path owns durability).
        publish_agent_delivery_event(AgentDeliveryEvent::Error(
            "publish_without_subscriber_is_silent".to_string(),
        ));
    }
}
